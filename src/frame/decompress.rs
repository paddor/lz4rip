use std::{
    fmt,
    hash::Hasher,
    io::{self, BufRead},
    mem::size_of,
};
use twox_hash::XxHash32;

use super::Error;
use super::header::{
    BlockInfo, BlockMode, FrameInfo, MAGIC_NUMBER_SIZE, MAX_FRAME_INFO_SIZE, MIN_FRAME_INFO_SIZE,
};
use lz4rip_core::{SliceSink, WINDOW_SIZE};
use lz4rip_decode::decompress_into_sink_with_dict;

fn vec_sink_for_decompression(
    vec: &mut Vec<u8>,
    offset: usize,
    pos: usize,
    required_capacity: usize,
) -> SliceSink<'_> {
    vec.resize(offset + required_capacity, 0);
    SliceSink::new(&mut vec[offset..], pos)
}

/// Options for [`FrameDecoder`].
#[derive(Debug, Default, Clone, Copy)]
pub struct FrameDecoderOptions<'a> {
    /// External dictionary bytes and Dict_ID.
    pub dictionary: Option<(&'a [u8], u32)>,
    /// Total decompressed output size limit across all frames.
    pub max_output: Option<usize>,
}

/// A reader for decompressing the LZ4 frame format
///
/// This Decoder wraps any other reader that implements `io::Read`.
/// Bytes read will be decompressed according to the [LZ4 frame format](
/// https://github.com/lz4/lz4/blob/dev/doc/lz4_Frame_format.md).
///
/// # Example 1
/// Reading decompressed data from a file.
///
/// ```no_run
/// use std::io::Read;
/// let compressed_input = std::fs::File::open("datafile").unwrap();
/// let mut decompressor = lz4rip::frame::FrameDecoder::new(compressed_input);
/// let mut output = String::new();
/// decompressor.read_to_string(&mut output).unwrap();
/// ```
///
/// # Example 2
/// Reading decompressed data line by line.
///
/// ```no_run
/// use std::io::BufRead;
/// let compressed_input = std::fs::File::open("datafile").unwrap();
/// let mut decompressor = lz4rip::frame::FrameDecoder::new(compressed_input);
/// for line in decompressor.lines() {
///     println!("{}", line.unwrap());
/// }
/// ```
pub struct FrameDecoder<R: io::Read> {
    r: R,
    current_frame_info: Option<FrameInfo>,
    content_hasher: XxHash32,
    content_len: u64,
    src: Vec<u8>,
    dst: Vec<u8>,
    ext_dict_offset: usize,
    ext_dict_len: usize,
    dst_start: usize,
    dst_end: usize,
    dict: Vec<u8>,
    expected_dict_id: Option<u32>,
    max_output: usize,
    bytes_output: usize,
}

impl<R: io::Read> FrameDecoder<R> {
    /// Creates a new Decoder for the specified reader.
    pub fn new(rdr: R) -> FrameDecoder<R> {
        FrameDecoder {
            r: rdr,
            src: Default::default(),
            dst: Default::default(),
            ext_dict_offset: 0,
            ext_dict_len: 0,
            dst_start: 0,
            dst_end: 0,
            current_frame_info: None,
            content_hasher: XxHash32::with_seed(0),
            content_len: 0,
            dict: Vec::new(),
            expected_dict_id: None,
            max_output: usize::MAX,
            bytes_output: 0,
        }
    }

    /// Creates a new Decoder that decodes frames using the supplied external dictionary.
    pub fn with_dictionary(rdr: R, dict: &[u8], dict_id: u32) -> FrameDecoder<R> {
        Self::with_options(
            rdr,
            FrameDecoderOptions {
                dictionary: Some((dict, dict_id)),
                ..Default::default()
            },
        )
    }

    /// Creates a new Decoder with explicit options.
    pub fn with_options(rdr: R, options: FrameDecoderOptions<'_>) -> FrameDecoder<R> {
        let mut dec = Self::new(rdr);
        if let Some((dict, dict_id)) = options.dictionary {
            dec.dict = dict.to_vec();
            dec.expected_dict_id = Some(dict_id);
        }
        if let Some(max_output) = options.max_output {
            dec.max_output = max_output;
        }
        dec
    }

    /// Gets a reference to the underlying reader in this decoder.
    pub fn get_ref(&self) -> &R {
        &self.r
    }

    /// Gets a mutable reference to the underlying reader in this decoder.
    pub fn get_mut(&mut self) -> &mut R {
        &mut self.r
    }

    /// Consumes the FrameDecoder and returns the underlying reader.
    pub fn into_inner(self) -> R {
        self.r
    }

    fn read_frame_info(&mut self) -> Result<usize, io::Error> {
        let mut buffer = [0u8; MAX_FRAME_INFO_SIZE];

        match self.r.read(&mut buffer[..MAGIC_NUMBER_SIZE])? {
            0 => return Ok(0),
            MAGIC_NUMBER_SIZE => (),
            read => self.r.read_exact(&mut buffer[read..MAGIC_NUMBER_SIZE])?,
        }

        self.r
            .read_exact(&mut buffer[MAGIC_NUMBER_SIZE..MIN_FRAME_INFO_SIZE])?;
        let required = FrameInfo::read_size(&buffer[..MIN_FRAME_INFO_SIZE])?;
        if required != MIN_FRAME_INFO_SIZE {
            self.r
                .read_exact(&mut buffer[MIN_FRAME_INFO_SIZE..required])?;
        }

        let frame_info = FrameInfo::read(&buffer[..required])?;
        match (frame_info.dict_id, self.expected_dict_id) {
            (None, None) => {}
            (Some(_), None) => return Err(Error::DictionaryNotSupported.into()),
            (None, Some(_)) => return Err(Error::DictionaryRequired.into()),
            (Some(actual), Some(expected)) if actual != expected => {
                return Err(Error::DictIdMismatch { expected, actual }.into());
            }
            (Some(_), Some(_)) => {}
        }

        let max_block_size = frame_info.block_size.get_size();
        let dst_size = if frame_info.block_mode == BlockMode::Linked {
            max_block_size * 2 + WINDOW_SIZE
        } else {
            max_block_size
        };
        self.src.clear();
        self.dst.clear();
        self.src.reserve_exact(max_block_size);
        self.dst.reserve_exact(dst_size);
        self.current_frame_info = Some(frame_info);
        self.content_hasher = XxHash32::with_seed(0);
        self.content_len = 0;
        self.ext_dict_len = 0;
        self.dst_start = 0;
        self.dst_end = 0;
        Ok(required)
    }

    fn add_output_len(&mut self, len: usize) -> io::Result<()> {
        let Some(actual) = self.bytes_output.checked_add(len) else {
            return Err(Error::DecompressedSizeLimit {
                limit: self.max_output,
                actual: usize::MAX,
            }
            .into());
        };
        if actual > self.max_output {
            return Err(Error::DecompressedSizeLimit {
                limit: self.max_output,
                actual,
            }
            .into());
        }
        self.bytes_output = actual;
        Ok(())
    }

    #[inline]
    fn read_checksum(r: &mut R) -> Result<u32, io::Error> {
        let mut checksum_buffer = [0u8; size_of::<u32>()];
        r.read_exact(&mut checksum_buffer[..])?;
        let checksum = u32::from_le_bytes(checksum_buffer);
        Ok(checksum)
    }

    #[inline]
    fn check_block_checksum(data: &[u8], expected_checksum: u32) -> Result<(), io::Error> {
        let mut block_hasher = XxHash32::with_seed(0);
        block_hasher.write(data);
        let calc_checksum = block_hasher.finish() as u32;
        if calc_checksum != expected_checksum {
            return Err(Error::BlockChecksumError.into());
        }
        Ok(())
    }

    fn read_block(&mut self) -> io::Result<usize> {
        debug_assert_eq!(self.dst_start, self.dst_end);
        let frame_info = *self
            .current_frame_info
            .as_ref()
            .ok_or_else(|| io::Error::other("no frame header has been read"))?;

        let max_block_size = frame_info.block_size.get_size();
        if frame_info.block_mode == BlockMode::Linked {
            debug_assert_eq!(self.dst.capacity(), max_block_size * 2 + WINDOW_SIZE);
            if self.dst_start + max_block_size > self.dst.capacity() {
                debug_assert!(self.dst_start >= max_block_size + WINDOW_SIZE);
                self.ext_dict_offset = self.dst_start - WINDOW_SIZE;
                self.ext_dict_len = WINDOW_SIZE;
                self.dst_start = 0;
                self.dst_end = 0;
            } else if self.dst_start + self.ext_dict_len > WINDOW_SIZE {
                let delta = self
                    .ext_dict_len
                    .min(self.dst_start + self.ext_dict_len - WINDOW_SIZE);
                self.ext_dict_offset += delta;
                self.ext_dict_len -= delta;
                debug_assert!(self.dst_start + self.ext_dict_len >= WINDOW_SIZE)
            }
        } else {
            debug_assert_eq!(self.ext_dict_len, 0);
            debug_assert_eq!(self.dst.capacity(), max_block_size);
            self.dst_start = 0;
            self.dst_end = 0;
        }

        let block_info = {
            let mut buffer = [0u8; 4];
            self.r.read_exact(&mut buffer)?;
            BlockInfo::read(&buffer)?
        };
        match block_info {
            BlockInfo::Uncompressed(len) => {
                let len = len as usize;
                if len > max_block_size {
                    return Err(Error::BlockTooBig.into());
                }
                self.add_output_len(len)?;
                self.r.read_exact(vec_resize_and_get_mut(
                    &mut self.dst,
                    self.dst_start,
                    self.dst_start + len,
                ))?;
                if frame_info.block_checksums {
                    let expected_checksum = Self::read_checksum(&mut self.r)?;
                    Self::check_block_checksum(
                        &self.dst[self.dst_start..self.dst_start + len],
                        expected_checksum,
                    )?;
                }

                self.dst_end += len;
                self.content_len += len as u64;
            }
            BlockInfo::Compressed(len) => {
                let len = len as usize;
                if len > max_block_size {
                    return Err(Error::BlockTooBig.into());
                }
                self.r
                    .read_exact(vec_resize_and_get_mut(&mut self.src, 0, len))?;
                if frame_info.block_checksums {
                    let expected_checksum = Self::read_checksum(&mut self.r)?;
                    Self::check_block_checksum(&self.src[..len], expected_checksum)?;
                }

                let with_dict_mode =
                    frame_info.block_mode == BlockMode::Linked && self.ext_dict_len != 0;
                let decomp_size = if with_dict_mode {
                    debug_assert!(self.dst_start + max_block_size <= self.ext_dict_offset);
                    let (head, tail) = self.dst.split_at_mut(self.ext_dict_offset);
                    let ext_dict = &tail[..self.ext_dict_len];

                    debug_assert!(head.len() - self.dst_start >= max_block_size);
                    decompress_into_sink_with_dict::<true>(
                        &self.src[..len],
                        &mut SliceSink::new(head, self.dst_start),
                        ext_dict,
                    )
                } else if !self.dict.is_empty() {
                    debug_assert!(self.dst.capacity() - self.dst_start >= max_block_size);
                    decompress_into_sink_with_dict::<true>(
                        &self.src[..len],
                        &mut vec_sink_for_decompression(
                            &mut self.dst,
                            0,
                            self.dst_start,
                            self.dst_start + max_block_size,
                        ),
                        &self.dict,
                    )
                } else {
                    debug_assert!(self.dst.capacity() - self.dst_start >= max_block_size);
                    decompress_into_sink_with_dict::<false>(
                        &self.src[..len],
                        &mut vec_sink_for_decompression(
                            &mut self.dst,
                            0,
                            self.dst_start,
                            self.dst_start + max_block_size,
                        ),
                        b"",
                    )
                }
                .map_err(Error::DecompressionError)?;

                self.add_output_len(decomp_size)?;
                self.dst_end += decomp_size;
                self.content_len += decomp_size as u64;
            }

            BlockInfo::EndMark => {
                if let Some(expected) = frame_info.content_size {
                    if self.content_len != expected {
                        return Err(Error::ContentLengthError {
                            expected,
                            actual: self.content_len,
                        }
                        .into());
                    }
                }
                if frame_info.content_checksum {
                    let expected_checksum = Self::read_checksum(&mut self.r)?;
                    let calc_checksum = self.content_hasher.finish() as u32;
                    if calc_checksum != expected_checksum {
                        return Err(Error::ContentChecksumError.into());
                    }
                }
                self.current_frame_info = None;
                return Ok(0);
            }
        }

        if frame_info.content_checksum {
            self.content_hasher
                .write(&self.dst[self.dst_start..self.dst_end]);
        }

        Ok(self.dst_end - self.dst_start)
    }

    fn read_more(&mut self) -> io::Result<usize> {
        loop {
            if self.current_frame_info.is_none() && self.read_frame_info()? == 0 {
                return Ok(0);
            }
            let read = self.read_block()?;
            if read != 0 || self.current_frame_info.is_some() {
                return Ok(read);
            }
        }
    }
}

impl<R: io::Read> io::Read for FrameDecoder<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.dst_start < self.dst_end {
                let read_len = std::cmp::min(self.dst_end - self.dst_start, buf.len());
                let dst_read_end = self.dst_start + read_len;
                buf[..read_len].copy_from_slice(&self.dst[self.dst_start..dst_read_end]);
                self.dst_start = dst_read_end;
                return Ok(read_len);
            }
            if self.read_more()? == 0 {
                return Ok(0);
            }
        }
    }

    fn read_to_end(&mut self, buf: &mut Vec<u8>) -> io::Result<usize> {
        let mut written = 0;
        loop {
            match self.fill_buf() {
                Ok([]) => return Ok(written),
                Ok(b) => {
                    buf.extend_from_slice(b);
                    let len = b.len();
                    self.consume(len);
                    written += len;
                }
                Err(ref e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
}

impl<R: io::Read> io::BufRead for FrameDecoder<R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.dst_start == self.dst_end {
            self.read_more()?;
        }
        Ok(&self.dst[self.dst_start..self.dst_end])
    }

    fn consume(&mut self, amt: usize) {
        assert!(amt <= self.dst_end - self.dst_start);
        self.dst_start += amt;
    }
}

impl<R: fmt::Debug + io::Read> fmt::Debug for FrameDecoder<R> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("FrameDecoder")
            .field("r", &self.r)
            .field("content_hasher", &self.content_hasher)
            .field("content_len", &self.content_len)
            .field("src", &"[...]")
            .field("dst", &"[...]")
            .field("dst_start", &self.dst_start)
            .field("dst_end", &self.dst_end)
            .field("ext_dict_offset", &self.ext_dict_offset)
            .field("ext_dict_len", &self.ext_dict_len)
            .field("current_frame_info", &self.current_frame_info)
            .field("max_output", &self.max_output)
            .field("bytes_output", &self.bytes_output)
            .finish()
    }
}

#[inline]
fn vec_resize_and_get_mut(v: &mut Vec<u8>, start: usize, end: usize) -> &mut [u8] {
    if end > v.len() {
        v.resize(end, 0)
    }
    &mut v[start..end]
}
