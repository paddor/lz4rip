use std::io::{Read, Write};

use wasm_bindgen::prelude::*;

const FLAG_HAS_DICTIONARY: u32 = 1 << 0;
const FLAG_LINKED_BLOCKS: u32 = 1 << 1;
const FLAG_BLOCK_CHECKSUMS: u32 = 1 << 2;
const FLAG_CONTENT_CHECKSUM: u32 = 1 << 3;
const FLAG_CONTENT_SIZE: u32 = 1 << 4;

#[wasm_bindgen(js_name = "compressBound")]
pub fn compress_bound(input_len: usize) -> usize {
    lz4rip::get_maximum_output_size(input_len)
}

#[wasm_bindgen]
pub fn compress(input: &[u8]) -> Vec<u8> {
    lz4rip::compress(input)
}

#[wasm_bindgen]
pub fn decompress(input: &[u8], uncompressed_size: usize) -> Result<Vec<u8>, JsError> {
    let output =
        lz4rip::decompress(input, uncompressed_size).map_err(|e| JsError::new(&format!("{e}")))?;
    validate_exact_size(output, uncompressed_size)
}

#[wasm_bindgen(js_name = "compressFrame")]
pub fn compress_frame(
    input: &[u8],
    dictionary: &[u8],
    dict_id: u32,
    flags: u32,
) -> Result<Vec<u8>, JsError> {
    let mut frame_info = lz4rip::frame::FrameInfo::new()
        .block_checksums(flags & FLAG_BLOCK_CHECKSUMS != 0)
        .content_checksum(flags & FLAG_CONTENT_CHECKSUM != 0);
    if flags & FLAG_LINKED_BLOCKS != 0 {
        frame_info = frame_info.block_mode(lz4rip::frame::BlockMode::Linked);
    }
    if flags & FLAG_CONTENT_SIZE != 0 {
        frame_info = frame_info.content_size(Some(input.len() as u64));
    }

    let mut encoder = if flags & FLAG_HAS_DICTIONARY != 0 {
        lz4rip::frame::FrameEncoder::with_dictionary(
            Vec::new(),
            dictionary,
            dict_id,
            Some(frame_info),
        )
        .map_err(|e| JsError::new(&format!("{e}")))?
    } else {
        lz4rip::frame::FrameEncoder::with_frame_info(frame_info, Vec::new())
    };
    encoder
        .write_all(input)
        .map_err(|e| JsError::new(&format!("{e}")))?;
    encoder.finish().map_err(|e| JsError::new(&format!("{e}")))
}

#[wasm_bindgen(js_name = "decompressFrame")]
pub fn decompress_frame(
    input: &[u8],
    dictionary: &[u8],
    dict_id: u32,
    has_dictionary: bool,
    max_output: usize,
    has_max_output: bool,
) -> Result<Vec<u8>, JsError> {
    let mut decoder = lz4rip::frame::FrameDecoder::with_options(
        input,
        lz4rip::frame::FrameDecoderOptions {
            dictionary: has_dictionary.then_some((dictionary, dict_id)),
            max_output: has_max_output.then_some(max_output),
        },
    );
    let mut output = Vec::new();
    decoder
        .read_to_end(&mut output)
        .map_err(|e| JsError::new(&format!("{e}")))?;
    Ok(output)
}

enum CompressorInner {
    Plain(lz4rip::block::Compressor),
    Dict(lz4rip::block::DictCompressor),
}

#[wasm_bindgen]
pub struct Compressor {
    inner: CompressorInner,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Compressor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Compressor {
        Compressor {
            inner: CompressorInner::Plain(lz4rip::block::Compressor::new()),
        }
    }

    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(dict: &[u8]) -> Compressor {
        Compressor {
            inner: CompressorInner::Dict(lz4rip::block::DictCompressor::new(dict)),
        }
    }

    pub fn compress(&mut self, input: &[u8]) -> Vec<u8> {
        match &mut self.inner {
            CompressorInner::Plain(c) => c.compress(input),
            CompressorInner::Dict(c) => c.compress(input),
        }
    }
}

#[wasm_bindgen]
pub struct Decompressor {
    inner: lz4rip::block::Decompressor,
}

impl Default for Decompressor {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl Decompressor {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Decompressor {
        Decompressor {
            inner: lz4rip::block::Decompressor::new(),
        }
    }

    #[wasm_bindgen(js_name = "withDict")]
    pub fn with_dict(dict: &[u8]) -> Decompressor {
        Decompressor {
            inner: lz4rip::block::Decompressor::with_dict(dict),
        }
    }

    pub fn decompress(
        &self,
        input: &[u8],
        uncompressed_size: usize,
    ) -> Result<Vec<u8>, JsError> {
        let output = self
            .inner
            .decompress(input, uncompressed_size)
            .map_err(|e| JsError::new(&format!("{e}")))?;
        validate_exact_size(output, uncompressed_size)
    }
}

#[wasm_bindgen]
pub struct DictTrainer {
    inner: Option<lz4rip::block::DictTrainer>,
}

#[wasm_bindgen]
impl DictTrainer {
    #[wasm_bindgen(constructor)]
    pub fn new(max_dict_size: usize) -> DictTrainer {
        DictTrainer {
            inner: Some(lz4rip::block::DictTrainer::new(max_dict_size)),
        }
    }

    #[wasm_bindgen(js_name = "addSample")]
    pub fn add_sample(&mut self, data: &[u8]) -> Result<(), JsError> {
        let trainer = self
            .inner
            .as_mut()
            .ok_or_else(|| JsError::new("DictTrainer already consumed by train()"))?;
        trainer.add_sample(data);
        Ok(())
    }

    #[wasm_bindgen(js_name = "sampleCount")]
    pub fn sample_count(&self) -> Result<usize, JsError> {
        self.inner
            .as_ref()
            .map(|t| t.sample_count())
            .ok_or_else(|| JsError::new("DictTrainer already consumed by train()"))
    }

    pub fn train(&mut self) -> Result<Vec<u8>, JsError> {
        let trainer = self
            .inner
            .take()
            .ok_or_else(|| JsError::new("DictTrainer already consumed by train()"))?;
        Ok(trainer.train())
    }
}

fn validate_exact_size(output: Vec<u8>, uncompressed_size: usize) -> Result<Vec<u8>, JsError> {
    if output.len() == uncompressed_size {
        Ok(output)
    } else {
        Err(JsError::new(&format!(
            "decompressed size mismatch: expected {uncompressed_size}, got {}",
            output.len()
        )))
    }
}
