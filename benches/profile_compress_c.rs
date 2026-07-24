use std::hint::black_box;

fn read_input(which: &str) -> Vec<u8> {
    let path = match which {
        "xml" => "corpus/silesia/xml",
        "dickens" => "corpus/silesia/dickens",
        "nci" => "corpus/silesia/nci",
        other => panic!("unknown input: {other}"),
    };
    std::fs::read(path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

fn main() {
    let which = std::env::args().nth(1).unwrap_or_else(|| "xml".into());
    let data = read_input(&which);
    let iters: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(10_000);
    let max_out = lzzzz::lz4::max_compressed_size(data.len());
    let mut output = vec![0u8; max_out];

    for _ in 0..iters {
        let n = lzzzz::lz4::compress(&data, &mut output, lzzzz::lz4::ACC_LEVEL_DEFAULT).unwrap();
        black_box(n);
    }
}
