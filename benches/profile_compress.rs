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
    let max_output = lz4rip::block::get_maximum_output_size(data.len());
    let mut output = vec![0u8; max_output];

    for _ in 0..iters {
        let n = lz4rip::compress_into(&data, &mut output).unwrap();
        black_box(n);
    }
}
