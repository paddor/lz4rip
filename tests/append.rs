use lz4rip::block::{
    DecompressError, Decompressor, DecompressorRef, DictCompressor, compress, decompress_append,
    decompress_into,
};

/// Mixes literal runs, runs with periods 1 to 20, and copies of earlier
/// output, so decoding takes every match-copy path.
fn mixed_payload(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        let r = next();
        match r % 4 {
            0 => {
                let n = 1 + (r >> 8) as usize % 80;
                out.extend((0..n).map(|_| next() as u8));
            }
            1 => {
                let period = 1 + (r >> 8) as usize % 20;
                let n = 4 + (r >> 16) as usize % 300;
                let pattern: Vec<u8> = (0..period).map(|_| next() as u8).collect();
                out.extend(pattern.iter().cycle().take(n));
            }
            _ if out.len() < 64 => out.extend_from_slice(b"the quick brown fox jumps over "),
            _ => {
                let n = 4 + (r >> 8) as usize % 200;
                let start = out.len() - 1 - (r >> 24) as usize % out.len().min(65_535);
                for i in 0..n {
                    out.push(out[start + i]);
                }
            }
        }
    }
    out.truncate(len);
    out
}

fn check_block(data: &[u8]) {
    let compressed = compress(data);

    // Exact size takes the slow tail paths; slack takes the wildcopy paths.
    for slack in [0, 64] {
        let mut buf = vec![0u8; data.len() + slack];
        let n = decompress_into(&compressed, &mut buf).unwrap();
        assert_eq!(&buf[..n], data);
    }

    let mut appended = b"prefix".to_vec();
    let n = decompress_append(&compressed, &mut appended, data.len()).unwrap();
    assert_eq!(n, data.len());
    assert_eq!(&appended[..6], b"prefix");
    assert_eq!(&appended[6..], data);
}

#[test]
fn into_and_append_match_input() {
    for (i, len) in [0, 1, 15, 16, 64, 300, 1000, 5000].into_iter().enumerate() {
        check_block(&mixed_payload(len, i as u64 + 1));
    }
}

#[test]
fn into_and_append_match_input_large() {
    check_block(&mixed_payload(70_000, 7));
}

#[test]
fn append_twice_concatenates() {
    let a = mixed_payload(700, 1);
    let b = mixed_payload(900, 2);
    let mut out = Vec::new();
    decompress_append(&compress(&a), &mut out, a.len()).unwrap();
    decompress_append(&compress(&b), &mut out, b.len()).unwrap();
    assert_eq!(&out[..a.len()], a);
    assert_eq!(&out[a.len()..], b);
}

#[test]
fn append_error_keeps_vec() {
    let data = mixed_payload(1000, 3);
    let compressed = compress(&data);
    let mut out = b"prefix".to_vec();

    let err = decompress_append(&compressed, &mut out, data.len() - 1).unwrap_err();
    assert!(matches!(err, DecompressError::OutputTooSmall { .. }));
    assert_eq!(out, b"prefix");

    let truncated = &compressed[..compressed.len() / 2];
    assert!(decompress_append(truncated, &mut out, data.len()).is_err());
    assert_eq!(out, b"prefix");
}

#[test]
fn append_does_not_reference_existing_bytes() {
    // Zero literals, then a match at offset 1 with nothing decoded before it.
    let block = [0x00, 0x01, 0x00];
    let mut out = b"abcd".to_vec();
    assert_eq!(
        decompress_append(&block, &mut out, 16),
        Err(DecompressError::OffsetOutOfBounds)
    );
    assert_eq!(out, b"abcd");
    assert_eq!(
        decompress_into(&block, &mut [0u8; 16]),
        Err(DecompressError::OffsetOutOfBounds)
    );
}

#[test]
fn append_errors_match_into() {
    let data = mixed_payload(1000, 4);
    let compressed = compress(&data);
    for (input, cap) in [
        (&compressed[..], data.len() - 1),
        (&compressed[..compressed.len() - 3], data.len()),
        (&[0x00, 0x00, 0x00][..], 16),
        (&[0x10][..], 16),
    ] {
        let expected = decompress_into(input, &mut vec![0u8; cap]).unwrap_err();
        let mut out = b"prefix".to_vec();
        assert_eq!(decompress_append(input, &mut out, cap), Err(expected));
        assert_eq!(out, b"prefix");
    }
}

#[test]
fn dict_into_and_append_match_input() {
    let corpus = mixed_payload(6000, 5);
    let (dict, input) = corpus.split_at(2048);
    // Starts with dictionary content, so early matches reach into the dict.
    let input = [&dict[1000..1500], input].concat();
    let compressed = DictCompressor::new(dict).compress(&input);

    let owned = Decompressor::with_dict(dict);
    let borrowed = DecompressorRef::with_dict(dict);

    let mut buf = vec![0u8; input.len()];
    assert_eq!(
        owned.decompress_into(&compressed, &mut buf),
        Ok(input.len())
    );
    assert_eq!(buf, input);
    let mut buf = vec![0u8; input.len() + 64];
    assert_eq!(
        borrowed.decompress_into(&compressed, &mut buf),
        Ok(input.len())
    );
    assert_eq!(&buf[..input.len()], input);

    let mut out = b"prefix".to_vec();
    assert_eq!(
        owned.decompress_append(&compressed, &mut out, input.len()),
        Ok(input.len())
    );
    assert_eq!(&out[6..], input);

    let mut out = Vec::new();
    assert_eq!(
        borrowed.decompress_append(&compressed, &mut out, input.len()),
        Ok(input.len())
    );
    assert_eq!(out, input);
}
