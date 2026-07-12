#[cfg(feature = "alloc")]
use alloc::boxed::Box;

/// Count matching bytes between `input[cur..]` and `source[candidate..]`,
/// stopping before `input[input_len - end_offset]`.
///
/// Uses `chunks_exact(8).zip(..)` to avoid a per-iteration bounds check and
/// autovectorize, then a byte tail. `to_le` makes `trailing_zeros` count from
/// the lowest-address mismatching byte.
#[inline]
pub(crate) fn count_same_bytes_inbounds(
    input: &[u8],
    cur: &mut usize,
    source: &[u8],
    candidate: usize,
    end_offset: usize,
) -> usize {
    const STEP: usize = 8;
    debug_assert!(*cur + end_offset <= input.len());
    debug_assert!(candidate <= source.len());
    let max_input = input.len() - *cur - end_offset;
    let max_cand = source.len() - candidate;
    let limit = max_input.min(max_cand);
    let cur_slice = &input[*cur..*cur + limit];
    let cand_slice = &source[candidate..candidate + limit];

    let mut num = 0;
    for (a, b) in cur_slice
        .chunks_exact(STEP)
        .zip(cand_slice.chunks_exact(STEP))
    {
        let av = u64::from_ne_bytes(a.try_into().unwrap());
        let bv = u64::from_ne_bytes(b.try_into().unwrap());
        if av == bv {
            num += STEP;
        } else {
            num += ((av ^ bv).to_le().trailing_zeros() / 8) as usize;
            *cur += num;
            return num;
        }
    }
    num += cur_slice[num..]
        .iter()
        .zip(&cand_slice[num..])
        .take_while(|(a, b)| a == b)
        .count();

    *cur += num;
    num
}

/// Read 4 bytes at position `n` (bounds-checked, native-endian).
#[inline]
pub(crate) fn get_batch_inbounds(input: &[u8], n: usize) -> u32 {
    u32::from_ne_bytes(input[n..n + 4].try_into().unwrap())
}

/// Read a usize-sized "batch" from some position (native-endian).
#[inline]
#[cfg(target_pointer_width = "64")]
pub(crate) fn get_batch_arch(input: &[u8], n: usize) -> usize {
    const USIZE_SIZE: usize = core::mem::size_of::<usize>();
    let arr: &[u8; USIZE_SIZE] = input[n..n + USIZE_SIZE].try_into().unwrap();
    usize::from_ne_bytes(*arr)
}

// Knuth's multiplicative hash constant (golden ratio * 2^32).
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
    /// Hash `input[pos..]`.
    fn get_hash_at(input: &[u8], pos: usize) -> usize;
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

#[cfg(target_pointer_width = "64")]
const U32_HASH_BYTES: usize = 5;

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
    #[cfg(target_pointer_width = "64")]
    fn get_hash_at(input: &[u8], pos: usize) -> usize {
        let batch = get_batch_arch(input, pos);
        (batch << 24).wrapping_mul(PRIME5) >> (64 - N.ilog2() as usize)
    }
    #[inline]
    #[cfg(target_pointer_width = "32")]
    fn get_hash_at(input: &[u8], pos: usize) -> usize {
        let batch = u32::from_ne_bytes(input[pos..pos + 4].try_into().unwrap());
        (batch.wrapping_mul(KNUTH) >> (32 - N.ilog2())) as usize
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
    #[cfg(target_pointer_width = "64")]
    fn get_hash_at(input: &[u8], pos: usize) -> usize {
        if U32_HASH_BYTES == 5 {
            let batch = get_batch_arch(input, pos);
            (batch << 24).wrapping_mul(PRIME5) >> (64 - N.ilog2() as usize)
        } else {
            let batch = u32::from_ne_bytes(input[pos..pos + 4].try_into().unwrap());
            (batch.wrapping_mul(KNUTH) >> (32 - N.ilog2())) as usize
        }
    }
    #[inline]
    #[cfg(target_pointer_width = "32")]
    fn get_hash_at(input: &[u8], pos: usize) -> usize {
        let batch = u32::from_ne_bytes(input[pos..pos + 4].try_into().unwrap());
        (batch.wrapping_mul(KNUTH) >> (32 - N.ilog2())) as usize
    }
}
