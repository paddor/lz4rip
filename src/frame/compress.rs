use std::{
    fmt,
    hash::Hasher,
    io::{self, Write},
};
use twox_hash::XxHash32;

use lz4rip_core::{SliceSink, WINDOW_SIZE};
use lz4rip_encode::{
    HashTableU32, compress_into_sink_with_dict, compress_into_sink_with_table,
    get_maximum_output_size, seed_table_with_input,
};

use super::Error;
use super::{
    BlockSize,
    header::{BLOCK_INFO_SIZE, BlockInfo, BlockMode, FrameInfo, MAX_FRAME_INFO_SIZE},
};

fn vec_sink_for_compression(
    vec: &mut Vec<u8>,
    offset: usize,
    pos: usize,
    required_capacity: usize,
) -> SliceSink<'_> {
    vec.resize(offset + required_capacity, 0);
    SliceSink::new(&mut vec[offset..], pos)
}

/// A writer for compressing a LZ4 stream.
///
/// This `FrameEncoder` wraps any other writer that implements `io::Write`.
/// Bytes written to this writer are compressed using the [LZ4 frame
/// format](https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md).
///
/// Writes are buffered automatically, so there's no need to wrap the given
/// writer in a `std::io::BufWriter`.
///
/// To ensure a well formed stream the encoder must be finalized by calling
/// either the [`finish()`], [`try_finish()`], or [`auto_finish()`] methods.
///
/// [`finish()`]: Self::finish
/// [`try_finish()`]: Self::try_finish
/// [`auto_finish()`]: Self::auto_finish
///
/// # Example 1
/// Compressing data into a file.
///
/// ```no_run
/// use std::io::Write;
/// let compressed_file = std::fs::File::create("datafile").unwrap();
/// let mut compressor = lz4rip::frame::FrameEncoder::new(compressed_file);
/// compressor.write_all(b"Hello, world!").unwrap();
/// compressor.finish().unwrap();
/// ```
///
/// # Example 2
/// Compressing multiple writes using linked blocks.
///
/// ```no_run
/// use std::io::Write;
/// let compressed_file = std::fs::File::create("datafile").unwrap();
/// let mut frame_info = lz4rip::frame::FrameInfo::new();
/// frame_info.block_mode = lz4rip::frame::BlockMode::Linked;
/// let mut compressor = lz4rip::frame::FrameEncoder::with_frame_info(frame_info, compressed_file);
/// for i in 0..10u64 {
///     write!(compressor, "record {i}\n").unwrap();
/// }
/// compressor.finish().unwrap();
/// ```
pub struct FrameEncoder<W: io::Write> {
    src: Vec<u8>,
    src_start: usize,
    src_end: usize,
    ext_dict_offset: usize,
    ext_dict_len: usize,
    src_stream_offset: usize,
    compression_table: HashTableU32,
    w: W,
    content_hasher: XxHash32,
    content_len: u64,
    dst: Vec<u8>,
    is_frame_open: bool,
    data_to_frame_written: bool,
    frame_info: FrameInfo,
    dict: Vec<u8>,
}

impl<W: io::Write> FrameEncoder<W> {
    fn init(&mut self) {
        let max_block_size = self.frame_info.block_size.get_size();
        let src_size = if self.frame_info.block_mode == BlockMode::Linked {
            max_block_size * 2 + WINDOW_SIZE
        } else {
            max_block_size
        };
        self.src
            .reserve(src_size.saturating_sub(self.src.capacity()));
        self.dst
            .reserve(get_maximum_output_size(max_block_size).saturating_sub(self.dst.capacity()));
    }

    /// Returns a wrapper around `self` that will finish the stream on drop.
    pub fn auto_finish(self) -> AutoFinishEncoder<W> {
        AutoFinishEncoder {
            encoder: Some(self),
        }
    }

    /// Creates a new Encoder with the specified FrameInfo.
    pub fn with_frame_info(frame_info: FrameInfo, wtr: W) -> Self {
        FrameEncoder {
            src: Vec::new(),
            w: wtr,
            compression_table: HashTableU32::new(),
            content_hasher: XxHash32::with_seed(0),
            content_len: 0,
            dst: Vec::new(),
            is_frame_open: false,
            data_to_frame_written: false,
            frame_info,
            src_start: 0,
            src_end: 0,
            ext_dict_offset: 0,
            ext_dict_len: 0,
            src_stream_offset: 0,
            dict: Vec::new(),
        }
    }

    /// Creates a new Encoder with the default settings.
    pub fn new(wtr: W) -> Self {
        Self::with_frame_info(Default::default(), wtr)
    }

    /// Creates a new Encoder that compresses every block using the supplied external
    /// dictionary.
    ///
    /// When `frame_info` is `None`, uses default settings with
    /// [`BlockMode::Independent`]. When `Some`, the supplied settings are used.
    pub fn with_dictionary(
        wtr: W,
        dict: &[u8],
        dict_id: u32,
        frame_info: Option<FrameInfo>,
    ) -> Result<Self, Error> {
        let mut frame_info = frame_info.unwrap_or_default();
        frame_info.dict_id = Some(dict_id);
        let mut enc = Self::with_frame_info(frame_info, wtr);
        enc.dict = dict.to_vec();
        Ok(enc)
    }

    /// The frame information used by this Encoder.
    pub fn frame_info(&self) -> &FrameInfo {
        &self.frame_info
    }

    /// Consumes this encoder, flushing internal buffer and writing stream terminator.
    pub fn finish(mut self) -> Result<W, Error> {
        self.try_finish()?;
        Ok(self.w)
    }

    /// Attempt to finish this output stream, flushing internal buffer and writing stream
    /// terminator.
    pub fn try_finish(&mut self) -> Result<(), Error> {
        match self.flush() {
            Ok(()) => {
                if !self.is_frame_open && self.data_to_frame_written {
                    return Ok(());
                }
                if !self.is_frame_open && !self.data_to_frame_written {
                    self.begin_frame(0)?;
                }
                self.end_frame()?;
                self.data_to_frame_written = true;
                Ok(())
            }
            Err(err) => Err(err.into()),
        }
    }

    /// Returns the underlying writer _without_ flushing the stream.
    pub fn into_inner(self) -> W {
        self.w
    }

    /// Gets a reference to the underlying writer in this encoder.
    pub fn get_ref(&self) -> &W {
        &self.w
    }

    /// Gets a reference to the underlying writer in this encoder.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.w
    }

    fn end_frame(&mut self) -> Result<(), Error> {
        debug_assert!(self.is_frame_open);
        self.is_frame_open = false;
        if let Some(expected) = self.frame_info.content_size {
            if expected != self.content_len {
                return Err(Error::ContentLengthError {
                    expected,
                    actual: self.content_len,
                });
            }
        }

        let mut block_info_buffer = [0u8; BLOCK_INFO_SIZE];
        BlockInfo::EndMark.write(&mut block_info_buffer[..])?;
        self.w.write_all(&block_info_buffer[..])?;
        if self.frame_info.content_checksum {
            let content_checksum = self.content_hasher.finish() as u32;
            self.w.write_all(&content_checksum.to_le_bytes())?;
        }

        Ok(())
    }

    fn begin_frame(&mut self, buf_len: usize) -> io::Result<()> {
        self.is_frame_open = true;
        if self.frame_info.block_size == BlockSize::Auto {
            self.frame_info.block_size = BlockSize::from_buf_length(buf_len);
        }
        self.init();
        let mut frame_info_buffer = [0u8; MAX_FRAME_INFO_SIZE];
        let size = self.frame_info.write(&mut frame_info_buffer)?;
        self.w.write_all(&frame_info_buffer[..size])?;

        if self.content_len != 0 {
            self.content_len = 0;
            self.src_stream_offset = 0;
            self.src.clear();
            self.src_start = 0;
            self.src_end = 0;
            self.ext_dict_len = 0;
            self.content_hasher = XxHash32::with_seed(0);
            self.compression_table.clear();
        }
        Ok(())
    }

    fn write_block(&mut self) -> io::Result<()> {
        debug_assert!(self.is_frame_open);
        let max_block_size = self.frame_info.block_size.get_size();
        debug_assert!(self.src_end - self.src_start <= max_block_size);

        if self.src_stream_offset + max_block_size + WINDOW_SIZE >= u32::MAX as usize / 2 {
            self.compression_table
                .reposition((self.src_stream_offset - self.ext_dict_len) as _);
            self.src_stream_offset = self.ext_dict_len;
        }

        let input = &self.src[..self.src_end];
        let src = &input[self.src_start..];

        let dst_required_size = get_maximum_output_size(src.len());
        let first_frame_block = self.content_len == 0;

        let compress_result = if !self.dict.is_empty()
            && (self.frame_info.block_mode == BlockMode::Independent || first_frame_block)
        {
            debug_assert_eq!(self.ext_dict_len, 0);
            compress_into_sink_with_dict::<true>(
                src,
                &mut vec_sink_for_compression(&mut self.dst, 0, 0, dst_required_size),
                &self.dict,
            )
        } else if self.ext_dict_len != 0 {
            debug_assert_eq!(self.frame_info.block_mode, BlockMode::Linked);
            compress_into_sink_with_table::<true, true, false, _>(
                input,
                self.src_start,
                &mut vec_sink_for_compression(&mut self.dst, 0, 0, dst_required_size),
                &mut self.compression_table,
                &self.src[self.ext_dict_offset..self.ext_dict_offset + self.ext_dict_len],
                self.src_stream_offset,
            )
        } else {
            compress_into_sink_with_table::<false, true, false, _>(
                input,
                self.src_start,
                &mut vec_sink_for_compression(&mut self.dst, 0, 0, dst_required_size),
                &mut self.compression_table,
                b"",
                self.src_stream_offset,
            )
        };

        let (block_info, block_data) = match compress_result.map_err(Error::CompressionError)? {
            comp_len if comp_len < src.len() => {
                (BlockInfo::Compressed(comp_len as _), &self.dst[..comp_len])
            }
            _ => (BlockInfo::Uncompressed(src.len() as _), src),
        };

        if self.frame_info.block_mode == BlockMode::Linked
            && !self.dict.is_empty()
            && first_frame_block
        {
            seed_table_with_input(&mut self.compression_table, src, self.src_stream_offset);
        }

        let mut block_info_buffer = [0u8; BLOCK_INFO_SIZE];
        block_info.write(&mut block_info_buffer[..])?;
        self.w.write_all(&block_info_buffer[..])?;
        self.w.write_all(block_data)?;
        if self.frame_info.block_checksums {
            let block_checksum = XxHash32::oneshot(0, block_data);
            self.w.write_all(&block_checksum.to_le_bytes())?;
        }

        if self.frame_info.content_checksum {
            self.content_hasher.write(src);
        }

        self.content_len += src.len() as u64;
        self.src_start += src.len();
        debug_assert_eq!(self.src_start, self.src_end);
        if self.frame_info.block_mode == BlockMode::Linked {
            debug_assert_eq!(self.src.capacity(), max_block_size * 2 + WINDOW_SIZE);
            if self.src_start >= max_block_size + WINDOW_SIZE {
                self.ext_dict_offset = self.src_end - WINDOW_SIZE;
                self.ext_dict_len = WINDOW_SIZE;
                self.src_stream_offset += self.src_end;
                self.src_start = 0;
                self.src_end = 0;
            } else if self.src_start + self.ext_dict_len > WINDOW_SIZE {
                let delta = self
                    .ext_dict_len
                    .min(self.src_start + self.ext_dict_len - WINDOW_SIZE);
                self.ext_dict_offset += delta;
                self.ext_dict_len -= delta;
                debug_assert!(self.src_start + self.ext_dict_len >= WINDOW_SIZE)
            }
            debug_assert!(
                self.ext_dict_len == 0 || self.src_start + max_block_size <= self.ext_dict_offset
            );
        } else {
            debug_assert_eq!(self.ext_dict_len, 0);
            debug_assert_eq!(self.src.capacity(), max_block_size);
            self.src_start = 0;
            self.src_end = 0;
            self.src_stream_offset += src.len();
        }
        debug_assert!(self.src_start <= self.src_end);
        debug_assert!(self.src_start + max_block_size <= self.src.capacity());
        Ok(())
    }
}

impl<W: io::Write> io::Write for FrameEncoder<W> {
    fn write(&mut self, mut buf: &[u8]) -> io::Result<usize> {
        if !self.is_frame_open && !buf.is_empty() {
            self.begin_frame(buf.len())?;
        }
        let buf_len = buf.len();
        while !buf.is_empty() {
            let src_filled = self.src_end - self.src_start;
            let max_fill_len = self.frame_info.block_size.get_size() - src_filled;
            if max_fill_len == 0 {
                self.write_block()?;
                debug_assert_eq!(self.src_end, self.src_start);
                continue;
            }

            let fill_len = max_fill_len.min(buf.len());
            vec_copy_overwriting(&mut self.src, self.src_end, &buf[..fill_len]);
            buf = &buf[fill_len..];
            self.src_end += fill_len;
        }
        Ok(buf_len)
    }

    fn flush(&mut self) -> io::Result<()> {
        if self.src_start != self.src_end {
            self.write_block()?;
        }
        Ok(())
    }
}

/// A wrapper around an [`FrameEncoder<W>`] that finishes the stream on drop.
pub struct AutoFinishEncoder<W: Write> {
    encoder: Option<FrameEncoder<W>>,
}

impl<W: io::Write> Drop for AutoFinishEncoder<W> {
    fn drop(&mut self) {
        if let Some(mut encoder) = self.encoder.take() {
            let _ = encoder.try_finish();
        }
    }
}

impl<W: Write> Write for AutoFinishEncoder<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("encoder already finished"))
            .and_then(|enc| enc.write(buf))
    }

    fn flush(&mut self) -> io::Result<()> {
        self.encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("encoder already finished"))
            .and_then(|enc| enc.flush())
    }
}

impl<W: fmt::Debug + io::Write> fmt::Debug for FrameEncoder<W> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FrameEncoder")
            .field("w", &self.w)
            .field("frame_info", &self.frame_info)
            .field("is_frame_open", &self.is_frame_open)
            .field("content_hasher", &self.content_hasher)
            .field("content_len", &self.content_len)
            .field("compression_table", &"{ ... }")
            .field("data_to_frame_written", &self.data_to_frame_written)
            .field("dst", &"[...]")
            .field("src", &"[...]")
            .field("src_start", &self.src_start)
            .field("src_end", &self.src_end)
            .field("ext_dict_offset", &self.ext_dict_offset)
            .field("ext_dict_len", &self.ext_dict_len)
            .field("src_stream_offset", &self.src_stream_offset)
            .finish()
    }
}

#[inline]
fn vec_copy_overwriting(target: &mut Vec<u8>, target_start: usize, src: &[u8]) {
    debug_assert!(target_start + src.len() <= target.capacity());

    let overwrite_len = (target.len() - target_start).min(src.len());
    target[target_start..target_start + overwrite_len].copy_from_slice(&src[..overwrite_len]);
    target.extend_from_slice(&src[overwrite_len..]);
}
