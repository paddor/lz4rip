#[cfg(feature = "alloc")]
use alloc::boxed::Box;

/// Count matching bytes between `input[*cur..]` and `source[candidate..]`,
/// stopping before `input[input.len() - end_offset]`, and advance `*cur`.
///
/// Compares 8 bytes at a time, then steps down 4/2/1 bytes. Both sides are
/// cut to one equal length up front, so the loop needs no bounds checks.
/// `to_le` makes `trailing_zeros` count from the lowest-address mismatch.
#[inline]
pub(crate) fn count_same_bytes(
    input: &[u8],
    cur: &mut usize,
    source: &[u8],
    candidate: usize,
    end_offset: usize,
) -> usize {
    let a = &input[*cur..input.len() - end_offset];
    let b = &source[candidate..];
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);

    let mut num = 0;
    let (ca, cb) = (a.chunks_exact(8), b.chunks_exact(8));
    let (mut ra, mut rb) = (ca.remainder(), cb.remainder());
    for (x, y) in ca.zip(cb) {
        let diff =
            u64::from_ne_bytes(x.try_into().unwrap()) ^ u64::from_ne_bytes(y.try_into().unwrap());
        if diff != 0 {
            num += (diff.to_le().trailing_zeros() / 8) as usize;
            *cur += num;
            return num;
        }
        num += 8;
    }

    if let (Some(x), Some(y)) = (ra.first_chunk::<4>(), rb.first_chunk::<4>()) {
        let diff = u32::from_ne_bytes(*x) ^ u32::from_ne_bytes(*y);
        if diff != 0 {
            num += (diff.to_le().trailing_zeros() / 8) as usize;
            *cur += num;
            return num;
        }
        num += 4;
        (ra, rb) = (&ra[4..], &rb[4..]);
    }
    if let (Some(x), Some(y)) = (ra.first_chunk::<2>(), rb.first_chunk::<2>()) {
        if x == y {
            num += 2;
            (ra, rb) = (&ra[2..], &rb[2..]);
        }
    }
    if let (Some(x), Some(y)) = (ra.first(), rb.first()) {
        if x == y {
            num += 1;
        }
    }

    *cur += num;
    num
}

/// Read 4 bytes at `pos` (native-endian).
#[inline(always)]
pub(crate) fn rd32(src: &[u8], pos: usize) -> u32 {
    u32::from_ne_bytes(*src[pos..].first_chunk::<4>().unwrap())
}

/// Read 8 bytes at `pos` (native-endian).
#[inline(always)]
pub(crate) fn rd64(src: &[u8], pos: usize) -> u64 {
    u64::from_ne_bytes(*src[pos..].first_chunk::<8>().unwrap())
}

/// `rd64(src, pos)` for a `pos` the caller knows is at most `limit`, where
/// the compiler sees `limit + 8 <= src.len()`. Reading at `pos.min(limit)`
/// turns both bounds checks into one conditional move.
#[inline(always)]
pub(crate) fn rd64_upto(src: &[u8], pos: usize, limit: usize) -> u64 {
    debug_assert!(pos <= limit);
    rd64(src, pos.min(limit))
}

/// The first four bytes in memory of eight bytes read with [`rd64`], as
/// [`rd32`] would read them.
#[cfg(target_pointer_width = "32")]
#[inline(always)]
pub(crate) fn first4(v: u64) -> u32 {
    let b = v.to_ne_bytes();
    u32::from_ne_bytes([b[0], b[1], b[2], b[3]])
}

// Knuth's multiplicative hash constant (golden ratio * 2^32).
#[cfg(target_pointer_width = "32")]
const KNUTH: u32 = 2_654_435_761;

#[cfg(target_pointer_width = "64")]
const PRIME5: usize = if cfg!(target_endian = "little") {
    889_523_592_379
} else {
    11_400_714_785_074_694_791
};

/// Hash table trait for LZ4 match finding.
pub(crate) trait HashTable {
    /// Look up a table entry by hash index.
    fn get_at(&self, idx: usize) -> usize;
    /// Store a position at the given hash index.
    fn put_at(&mut self, idx: usize, val: usize);
    /// Zero all entries.
    fn clear(&mut self);
    /// Hash the 8 bytes read with [`rd64`] at a position.
    fn hash(bytes: u64) -> usize;
    /// Hash `input[pos..]`. Reads 8 bytes.
    #[inline]
    fn get_hash_at(input: &[u8], pos: usize) -> usize {
        Self::hash(rd64(input, pos))
    }
}

/// Hash of the first five bytes in memory of `bytes` (64-bit), or the first
/// four (32-bit), onto `N` buckets.
#[inline(always)]
fn hash_n<const N: usize>(bytes: u64) -> usize {
    #[cfg(target_pointer_width = "64")]
    {
        ((bytes as usize) << 24).wrapping_mul(PRIME5) >> (64 - N.ilog2() as usize)
    }
    #[cfg(target_pointer_width = "32")]
    {
        (first4(bytes).wrapping_mul(KNUTH) >> (32 - N.ilog2())) as usize
    }
}

/// Default entry count for the no-dict (`u32`-valued) table: 2048 x 4 B = 8 KB.
pub const DEFAULT_NODICT_ENTRIES: usize = 2 * 1024;
/// Default entry count for the dict (`u16`-valued) tables: 4096 x 2 B = 8 KB.
pub const DEFAULT_DICT_ENTRIES: usize = 4 * 1024;
/// Smallest permitted hash-table entry count: 256 (an 8-bit index). Below this
/// the hash collapses 5 input bytes onto too few buckets to find matches, so the
/// compressor degrades to emitting literals. Matches C lz4's floor
/// (`LZ4_MEMORY_USAGE_MIN = 10` -> `1 << (10 - 2)` = 256-entry table).
pub const MIN_ENTRIES: usize = 256;

/// Compile-time validation of a hash-table entry count `N`.
///
/// `N` must be a power of two so the index shift `64 - N.ilog2()` maps the hash
/// onto exactly `[0, N)`, and at least [`MIN_ENTRIES`] so the shift is in range
/// and the table carries enough index bits to match.
const fn assert_valid_entries(n: usize) {
    assert!(
        n.is_power_of_two(),
        "hash table entry count must be a power of two"
    );
    assert!(
        n >= MIN_ENTRIES,
        "hash table entry count must be at least MIN_ENTRIES (256)"
    );
}

/// A hash table with `N` entries using 16-bit values (`2 * N` bytes).
///
/// `N` must be a power of two (checked at compile time in [`new`](Self::new)).
/// Stored positions must fit in `u16`, so this is used only when dict + input
/// stays below 64 KB.
#[derive(Debug)]
#[repr(align(64))]
pub(crate) struct HashTableU32U16<const N: usize = DEFAULT_DICT_ENTRIES> {
    #[cfg(feature = "alloc")]
    dict: Box<[u16; N]>,
    #[cfg(not(feature = "alloc"))]
    dict: [u16; N],
}
impl<const N: usize> HashTableU32U16<N> {
    #[cfg(feature = "alloc")]
    #[inline]
    pub(crate) fn new() -> Self {
        const { assert_valid_entries(N) };
        let dict = alloc::vec![0; N].into_boxed_slice().try_into().unwrap();
        Self { dict }
    }
    #[cfg(not(feature = "alloc"))]
    #[inline]
    pub(crate) fn new() -> Self {
        const { assert_valid_entries(N) };
        Self { dict: [0u16; N] }
    }
}
impl<const N: usize> HashTable for HashTableU32U16<N> {
    #[inline]
    fn get_at(&self, idx: usize) -> usize {
        self.dict[idx] as usize
    }
    #[inline]
    fn put_at(&mut self, idx: usize, val: usize) {
        self.dict[idx] = val as u16;
    }
    #[inline]
    fn clear(&mut self) {
        self.dict.fill(0);
    }
    #[inline]
    fn hash(bytes: u64) -> usize {
        hash_n::<N>(bytes)
    }
}

/// A hash table with `N` entries using 32-bit values (`4 * N` bytes).
///
/// `N` must be a power of two (checked at compile time in [`new`](Self::new)).
#[derive(Debug)]
pub struct HashTableU32<const N: usize = DEFAULT_NODICT_ENTRIES> {
    #[cfg(feature = "alloc")]
    dict: Box<[u32; N]>,
    #[cfg(not(feature = "alloc"))]
    dict: [u32; N],
}
impl<const N: usize> Default for HashTableU32<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> HashTableU32<N> {
    #[cfg(feature = "alloc")]
    #[inline]
    /// Create a new zeroed hash table.
    pub fn new() -> Self {
        const { assert_valid_entries(N) };
        let dict = alloc::vec![0; N].into_boxed_slice().try_into().unwrap();
        Self { dict }
    }
    #[cfg(not(feature = "alloc"))]
    #[inline]
    /// Create a new zeroed hash table.
    pub fn new() -> Self {
        const { assert_valid_entries(N) };
        Self { dict: [0u32; N] }
    }

    /// Zero all entries.
    #[inline]
    pub fn clear(&mut self) {
        self.dict.fill(0);
    }

    /// Subtract `offset` from all entries (saturating).
    #[cold]
    pub fn reposition(&mut self, offset: u32) {
        for i in self.dict.iter_mut() {
            *i = i.saturating_sub(offset);
        }
    }
}
impl<const N: usize> HashTable for HashTableU32<N> {
    #[inline]
    fn get_at(&self, idx: usize) -> usize {
        self.dict[idx] as usize
    }
    #[inline]
    fn put_at(&mut self, idx: usize, val: usize) {
        self.dict[idx] = val as u32;
    }
    #[inline]
    fn clear(&mut self) {
        self.dict.fill(0);
    }
    #[inline]
    fn hash(bytes: u64) -> usize {
        hash_n::<N>(bytes)
    }
}
