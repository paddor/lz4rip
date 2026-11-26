//! COVER dictionary trainer.

use alloc::collections::VecDeque;
use alloc::vec;
use alloc::vec::Vec;

use lz4rip_core::MAX_DISTANCE;
use lz4rip_core::MINMATCH;

/// Trains an LZ4 dictionary from sample messages using the COVER
/// algorithm.
///
/// # Example
/// ```
/// use lz4rip_encode::DictTrainer;
///
/// let mut trainer = DictTrainer::new(2048);
/// for msg in &[b"hello world" as &[u8], b"hello rust", b"hello lz4"] {
///     trainer.add_sample(msg);
/// }
/// let dict = trainer.train();
/// let compressor = lz4rip_encode::DictCompressor::new(&dict);
/// ```
pub struct DictTrainer {
    max_dict_size: usize,
    samples: VecDeque<Vec<u8>>,
    total_bytes: usize,
}

impl core::fmt::Debug for DictTrainer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("DictTrainer")
            .field("max_dict_size", &self.max_dict_size)
            .field("samples", &self.samples.len())
            .field("total_bytes", &self.total_bytes)
            .finish()
    }
}

impl DictTrainer {
    /// Create a trainer targeting `max_dict_size` bytes of output.
    ///
    /// Typical values: 2048 for small messages, 4096 for larger ones.
    /// The dict is capped at 65535 bytes (LZ4 max match distance).
    pub fn new(max_dict_size: usize) -> Self {
        let max_dict_size = max_dict_size.min(MAX_DISTANCE);
        DictTrainer {
            max_dict_size,
            samples: VecDeque::new(),
            total_bytes: 0,
        }
    }

    /// Add a training sample.
    ///
    /// Samples shorter than 4 bytes or longer than `max_dict_size` are
    /// silently skipped.
    pub fn add_sample(&mut self, data: &[u8]) {
        if data.len() < MINMATCH || data.len() > self.max_dict_size {
            return;
        }

        let budget = self.max_dict_size * 8;

        while self.total_bytes + data.len() > budget && !self.samples.is_empty() {
            self.total_bytes -= self.samples.pop_front().unwrap().len();
        }

        self.total_bytes += data.len();
        self.samples.push_back(data.to_vec());
    }

    /// Number of samples added so far.
    pub fn sample_count(&self) -> usize {
        self.samples.len()
    }

    /// Total bytes of sample data added so far.
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Train a dictionary from the collected samples. Consumes the
    /// trainer, freeing all sample data.
    ///
    /// Returns a raw byte buffer. If fewer than 2 samples were added,
    /// returns an empty vec.
    pub fn train(self) -> Vec<u8> {
        if self.samples.len() < 2 {
            return Vec::new();
        }
        // Move the samples into one buffer, freeing each once copied, so
        // training holds the sample data once.
        let mut concat = Vec::with_capacity(self.total_bytes);
        let mut offsets = Vec::with_capacity(self.samples.len() + 1);
        for sample in self.samples {
            offsets.push(concat.len());
            concat.extend_from_slice(&sample);
        }
        offsets.push(concat.len());
        cover_select(concat, &offsets, self.max_dict_size)
    }
}

const D: usize = 8;
const FREQ_BITS: usize = 16;
const FREQ_SIZE: usize = 1 << FREQ_BITS;
const FREQ_MASK: usize = FREQ_SIZE - 1;

#[inline(always)]
fn hash_dmer(data: &[u8]) -> usize {
    let v = u64::from_le_bytes(data[..8].try_into().unwrap());
    v.wrapping_mul(0x9E37_79B9_7F4A_7C15) as usize
}

/// Picks the dict from the samples in `concat`, sample `s` at
/// `offsets[s]..offsets[s + 1]`.
fn cover_select(mut concat: Vec<u8>, offsets: &[usize], dict_size: usize) -> Vec<u8> {
    if concat.len() < D {
        concat.truncate(dict_size);
        return concat;
    }

    let num_dmers = concat.len() - D + 1;

    let mut hashes = vec![0u32; num_dmers];
    for i in 0..num_dmers {
        hashes[i] = (hash_dmer(&concat[i..i + D]) & FREQ_MASK) as u32;
    }

    let mut freqs = vec![0u32; FREQ_SIZE];
    for s in 0..offsets.len() - 1 {
        let start = offsets[s];
        let end = offsets[s + 1];
        if end - start < D {
            continue;
        }
        for i in start..end - D + 1 {
            freqs[hashes[i] as usize] += 1;
        }
    }

    // A segment holds at least one d-mer, as COVER's `k >= d`. Below 32 bytes, segments are
    // one d-mer each, and the dict is cut to `dict_size` below.
    let k = (dict_size / 4).max(D);
    if concat.len() < k {
        return concat;
    }

    let seg_dmers = k - D + 1;
    let mut used = vec![false; concat.len()];
    let mut segments: Vec<(usize, u64)> = Vec::new();
    let mut collected = 0usize;

    let freq = |freqs: &[u32], i: usize| u64::from(freqs[hashes[i] as usize]);
    while collected < dict_size {
        // A segment's score is its d-mers' frequency sum, kept for a window
        // sliding over every start position.
        let mut score: u64 = (0..seg_dmers).map(|i| freq(&freqs, i)).sum();
        let mut best_pos = 0;
        let mut best_score = 0u64;
        for (pos, &pos_used) in used[..=concat.len() - k].iter().enumerate() {
            if pos > 0 {
                score = score + freq(&freqs, pos + seg_dmers - 1) - freq(&freqs, pos - 1);
            }
            if !pos_used && score > best_score {
                best_score = score;
                best_pos = pos;
            }
        }

        if best_score == 0 {
            break;
        }

        segments.push((best_pos, best_score));

        for i in best_pos..best_pos + seg_dmers {
            freqs[hashes[i] as usize] = 0;
        }
        used[best_pos..best_pos + k].fill(true);

        collected += k;
    }

    let mut dict = Vec::with_capacity(dict_size);
    for &(pos, _) in segments.iter().rev() {
        let end = (pos + k).min(concat.len());
        dict.extend_from_slice(&concat[pos..end]);
        if dict.len() >= dict_size {
            break;
        }
    }
    dict.truncate(dict_size);
    dict
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_msg(i: u32) -> Vec<u8> {
        alloc::format!(
            r#"{{"ts":"2026-04-27T12:00:00.{i:04}Z","level":"INFO","service":"api-gw","trace":"{i:08x}","method":"GET","path":"/v1/users/{i:04}","status":200,"latency_ms":{lat},"region":"us-east-1"}}"#,
            i = i,
            lat = 10 + i % 490,
        )
        .into_bytes()
    }

    #[test]
    fn train_produces_nonempty_dict() {
        let mut trainer = DictTrainer::new(2048);
        for i in 0..100 {
            trainer.add_sample(&json_msg(i));
        }
        let dict = trainer.train();
        assert!(!dict.is_empty(), "dict should not be empty");
        assert!(dict.len() <= 2048, "dict should respect max size");
    }

    #[test]
    fn skips_too_short() {
        let mut trainer = DictTrainer::new(2048);
        trainer.add_sample(b"hi");
        assert_eq!(trainer.sample_count(), 0);
    }

    #[test]
    fn skips_too_long() {
        let mut trainer = DictTrainer::new(64);
        trainer.add_sample(&[0u8; 100]);
        assert_eq!(trainer.sample_count(), 0);
    }

    #[test]
    fn evicts_old_samples() {
        let mut trainer = DictTrainer::new(2048);
        for i in 0..200 {
            trainer.add_sample(&json_msg(i));
        }
        assert!(trainer.total_bytes() <= 2048 * 8);
        assert!(trainer.sample_count() < 200);
    }

    #[test]
    fn too_few_samples_returns_empty() {
        let mut trainer = DictTrainer::new(2048);
        trainer.add_sample(b"hello world");
        assert!(trainer.train().is_empty());
    }

    #[test]
    fn small_dict_sizes_train_within_their_size() {
        let text = b"the quick brown fox jumps over the lazy dog, the quick brown fox ";
        for size in 0..=64 {
            let mut trainer = DictTrainer::new(size);
            let len = size.min(24);
            for i in 0..20 {
                if len >= MINMATCH {
                    trainer.add_sample(&text[i..i + len]);
                }
            }
            let dict = trainer.train();
            assert!(
                dict.len() <= size,
                "size {size}: dict of {} bytes",
                dict.len()
            );
            if len >= D {
                assert!(!dict.is_empty(), "size {size}: empty dict");
            }
        }
    }
}
