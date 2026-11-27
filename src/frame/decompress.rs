use std::{fmt, hash::Hasher, io, mem::size_of};
use twox_hash::XxHash32;

use super::Error;
use super::header::{
    BlockInfo, BlockMode, FrameInfo, MAGIC_NUMBER_SIZE, MAX_FRAME_INFO_SIZE, MIN_FRAME_INFO_SIZE,
};
use lz4rip_core::{DecompressError, SliceSink, WINDOW_SIZE};
use lz4rip_decode::decompress_into_sink_with_dict;

/// `read_to_end` trusts a frame's declared content size for up to this many
/// times the output the frame has decoded, and for all of it once that is
/// within this factor again of it. A false header then costs at most 256x
/// what really decoded. A large frame of the declared size grows a few
/// times, and its last growth copies at most 1/16 of it.
const RESERVE_EXPANSION: usize = 16;

/// In a frame with no declared content size, a compressed block first
/// decodes into at most this many times its length, unless more of the
/// output buffer is zeroed already. A new decoder zeroes its buffer up to the
/// limit, and zeroing 64 KiB took longer than decoding a 4 KiB block. A
/// block that decodes to more is decoded again with the full limit. 8x
/// covered every Silesia slice up to 16 KiB; 4x missed often enough on xml
/// and nci to be slower. A limit past half the block size takes the whole
/// block (see [`fill_up`]).
const FIRST_LIMIT_EXPANSION: usize = 8;

/// Returns a sink over `vec[..end]` that writes from `pos`. `vec` never
/// shrinks, so later blocks and frames reuse bytes that were already zeroed.
fn vec_sink_for_decompression(
    vec: &mut Vec<u8>,
    pos: usize,
    end: usize,
    capacity: impl FnOnce(usize) -> usize,
) -> SliceSink<'_> {
    grow_zeroed(vec, pos, end, capacity);
    SliceSink::new(&mut vec[..end], pos)
}

/// Zero-extends `vec` to `end` bytes, for the caller to write from `start`.
/// A buffer that grows takes `capacity(end)` bytes (see [`grown_capacity`]).
/// With nothing before `start` to keep, it is replaced instead of
/// reallocated, which would copy bytes that are only overwritten: an old
/// compressed block, or a failed first decode.
#[inline]
fn grow_zeroed(vec: &mut Vec<u8>, start: usize, end: usize, capacity: impl FnOnce(usize) -> usize) {
    if vec.len() < end {
        if end > vec.capacity() {
            let capacity = capacity(end).max(end);
            if start == 0 {
                // Freed first, so the allocator can reuse its memory.
                drop(core::mem::take(vec));
            }
            vec.reserve_exact(capacity - vec.len());
        }
        vec.resize(end, 0);
    }
}

/// The `src` and `dst` capacity whole blocks of the frame take, at most its
/// declared content size.
fn block_buffer_sizes(frame_info: &FrameInfo) -> (usize, usize) {
    let max_block_size = frame_info.block_size.get_size();
    let dst_size = if frame_info.block_mode == BlockMode::Linked {
        max_block_size * 2 + WINDOW_SIZE
    } else {
        max_block_size
    };
    let content_size = declared_size(frame_info);
    (max_block_size.min(content_size), dst_size.min(content_size))
}

/// The capacity a growing buffer takes for `end` bytes, given its `limit`
/// from [`block_buffer_sizes`]. During a frame's first block, what the block
/// needs (see [`fill_up`]), so a frame of one small message stays small. A
/// stream takes the limit at once and so reallocates once: a frame from its
/// second block, or from its first when it declares more content than one
/// block. The decode limit trusts that size for one block already.
///
/// NOTE: in a new decoder, 256 KiB frames decode 1-6% slower than when both
/// buffers were reserved at the header, with nearly the same instructions:
/// 4K aliasing follows where the buffers land. Doubling `src` in the first
/// block, to skip its regrowth, made 64 KiB frames up to 23% slower (glibc
/// heap trims), so this stays.
fn grown_capacity(frame_info: &FrameInfo, limit: usize, content_len: u64, end: usize) -> usize {
    let max_block_size = frame_info.block_size.get_size() as u64;
    let stream = content_len != 0
        || frame_info
            .content_size
            .is_some_and(|size| size > max_block_size);
    if stream { limit } else { fill_up(end, limit) }
}

/// What `src` takes to grow to `end` bytes (see [`grown_capacity`]).
fn src_capacity(frame_info: &FrameInfo, content_len: u64, end: usize) -> usize {
    grown_capacity(
        frame_info,
        block_buffer_sizes(frame_info).0,
        content_len,
        end,
    )
}

/// What `dst` takes to grow to `end` bytes (see [`grown_capacity`]).
fn dst_capacity(frame_info: &FrameInfo, content_len: u64, end: usize) -> usize {
    grown_capacity(
        frame_info,
        block_buffer_sizes(frame_info).1,
        content_len,
        end,
    )
}

/// `size`, or `limit` from half of it on. Blocks before a frame's last fill
/// the block size, so a block that needs more than half of it likely needs
/// all of it, and taking it at once saves growing again.
fn fill_up(size: usize, limit: usize) -> usize {
    if size >= limit / 2 {
        limit.max(size)
    } else {
        size
    }
}

/// Returns the frame's declared content size, or `usize::MAX` when it
/// declares none or more than fits in memory.
fn declared_size(frame_info: &FrameInfo) -> usize {
    frame_info
        .content_size
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(usize::MAX)
}

/// Reserves `buf` for a decoded block of `block_len` bytes. In a frame that
/// declares its content size, it also reserves the rest of the frame (see
/// [`RESERVE_EXPANSION`]; `decoded` includes this block). A frame of the
/// declared size ends with exactly that capacity. `allowed` caps it at the
/// output limit left.
fn reserve_output(
    buf: &mut Vec<u8>,
    block_len: usize,
    declared: Option<u64>,
    decoded: u64,
    allowed: usize,
) {
    if buf.capacity() - buf.len() >= block_len {
        return;
    }
    let Some(declared) = declared else {
        buf.reserve(block_len);
        return;
    };
    let rest = usize::try_from(declared.saturating_sub(decoded)).unwrap_or(usize::MAX);
    let proven = usize::try_from(decoded).unwrap_or(usize::MAX);
    let wanted = rest.min(allowed).saturating_add(block_len);
    let trusted = proven.saturating_mul(RESERVE_EXPANSION);
    let reserve = if trusted.saturating_mul(RESERVE_EXPANSION) >= wanted {
        wanted
    } else {
        trusted.max(block_len)
    };
    buf.reserve_exact(reserve);
}

/// Reserves `buf` for a block decoded straight into it, of at most `limit`
/// bytes. In a frame that declares its content size, it also reserves the
/// rest of the frame, as [`reserve_output`] does, trusting the declared size
/// as far as the `decoded` bytes before this block prove it. `allowed` caps
/// it at the output limit left.
fn reserve_block(
    buf: &mut Vec<u8>,
    limit: usize,
    frame_info: &FrameInfo,
    decoded: u64,
    allowed: usize,
) {
    if buf.capacity() - buf.len() >= limit {
        return;
    }
    let Some(declared) = frame_info.content_size else {
        buf.reserve(limit);
        return;
    };
    let rest = usize::try_from(declared.saturating_sub(decoded)).unwrap_or(usize::MAX);
    let proven = usize::try_from(decoded).unwrap_or(usize::MAX);
    let wanted = rest.min(allowed).max(limit);
    let trusted = proven.saturating_mul(RESERVE_EXPANSION);
    let reserve = if trusted.saturating_mul(RESERVE_EXPANSION) >= wanted {
        wanted
    } else {
        trusted.max(limit)
    };
    buf.reserve_exact(reserve);
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
/// After a read error, the decoder abandons the current frame state. The next
/// read expects a frame header at the reader's current position. Replacing the
/// reader through [`get_mut`](Self::get_mut) allows decoding a new frame, while
/// [`FrameDecoderOptions::max_output`] still counts all earlier output.
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

        self.current_frame_info = Some(frame_info);
        self.content_hasher = XxHash32::with_seed(0);
        self.content_len = 0;
        self.ext_dict_len = 0;
        self.dst_start = 0;
        self.dst_end = 0;
        Ok(required)
    }

    /// Returns the total output after `len` more bytes, or an error if it
    /// exceeds `max_output`.
    fn check_output_len(&self, len: usize) -> io::Result<usize> {
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
        Ok(actual)
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

    /// Reads the next block header.
    fn read_block_info(&mut self) -> io::Result<BlockInfo> {
        let mut buffer = [0u8; 4];
        self.r.read_exact(&mut buffer)?;
        Ok(BlockInfo::read(&buffer)?)
    }

    /// Reads a compressed block of `len` bytes into `src[..len]` and checks
    /// its checksum.
    fn read_compressed(&mut self, frame_info: &FrameInfo, len: usize) -> io::Result<()> {
        if len > frame_info.block_size.get_size() {
            return Err(Error::BlockTooBig.into());
        }
        let content_len = self.content_len;
        self.r
            .read_exact(vec_resize_and_get_mut(&mut self.src, 0, len, |end| {
                src_capacity(frame_info, content_len, end)
            }))?;
        if frame_info.block_checksums {
            let expected_checksum = Self::read_checksum(&mut self.r)?;
            Self::check_block_checksum(&self.src[..len], expected_checksum)?;
        }
        Ok(())
    }

    /// The first decode limit for the compressed block in `src[..len]`, with
    /// `zeroed` bytes of output already zeroed.
    ///
    /// Decode at most the bytes the frame still declares, so a small frame
    /// zeroes no more than its content. With no declared size, guess from
    /// the block's length (`FIRST_LIMIT_EXPANSION`). A block that fails
    /// within the first limit is decoded again with the full block limit, so
    /// errors, including the length reported at the end mark, match an
    /// unlimited decode. NOTE: a declared size is trusted for one block of
    /// zeroing (at most 4 MiB) before the data proves it. Kept: a smaller
    /// first limit would decode compressible blocks twice.
    fn first_limit(&self, frame_info: &FrameInfo, len: usize, zeroed: usize) -> usize {
        let max_block_size = frame_info.block_size.get_size();
        match frame_info.content_size {
            Some(size) => {
                usize::try_from(size.saturating_sub(self.content_len)).unwrap_or(usize::MAX)
            }
            None => fill_up(len.saturating_mul(FIRST_LIMIT_EXPANSION), max_block_size).max(zeroed),
        }
        .min(max_block_size)
    }

    /// Checks a frame's end mark: its declared length and content checksum.
    fn end_frame(&mut self, frame_info: &FrameInfo) -> io::Result<()> {
        if let Some(expected) = frame_info.content_size
            && self.content_len != expected
        {
            return Err(Error::ContentLengthError {
                expected,
                actual: self.content_len,
            }
            .into());
        }
        if frame_info.content_checksum {
            let expected_checksum = Self::read_checksum(&mut self.r)?;
            let calc_checksum = self.content_hasher.finish() as u32;
            if calc_checksum != expected_checksum {
                return Err(Error::ContentChecksumError.into());
            }
        }
        self.current_frame_info = None;
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
            let dst_size = max_block_size * 2 + WINDOW_SIZE;
            if self.dst_start + max_block_size > dst_size {
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
            self.dst_start = 0;
            self.dst_end = 0;
        }

        let block_info = self.read_block_info()?;
        match block_info {
            BlockInfo::Uncompressed(len) => {
                let len = len as usize;
                if len > max_block_size {
                    return Err(Error::BlockTooBig.into());
                }
                let bytes_output = self.check_output_len(len)?;
                let content_len = self.content_len;
                self.r.read_exact(vec_resize_and_get_mut(
                    &mut self.dst,
                    self.dst_start,
                    self.dst_start + len,
                    |end| dst_capacity(&frame_info, content_len, end),
                ))?;
                if frame_info.block_checksums {
                    let expected_checksum = Self::read_checksum(&mut self.r)?;
                    Self::check_block_checksum(
                        &self.dst[self.dst_start..self.dst_start + len],
                        expected_checksum,
                    )?;
                }

                self.bytes_output = bytes_output;
                self.dst_end += len;
                self.content_len += len as u64;
            }
            BlockInfo::Compressed(len) => {
                let len = len as usize;
                self.read_compressed(&frame_info, len)?;
                let mut limit = self.first_limit(
                    &frame_info,
                    len,
                    self.dst.len().saturating_sub(self.dst_start),
                );
                let decomp_size = loop {
                    match self.decompress_block(len, limit) {
                        Err(_) if limit < max_block_size => limit = max_block_size,
                        result => break result,
                    }
                }
                .map_err(Error::DecompressionError)?;

                self.bytes_output = self.check_output_len(decomp_size)?;
                self.dst_end += decomp_size;
                self.content_len += decomp_size as u64;
            }

            BlockInfo::EndMark => {
                self.end_frame(&frame_info)?;
                return Ok(0);
            }
        }

        if frame_info.content_checksum {
            self.content_hasher
                .write(&self.dst[self.dst_start..self.dst_end]);
        }

        Ok(self.dst_end - self.dst_start)
    }

    /// Decodes the compressed block in `src[..len]` to `dst_start`, writing
    /// at most `limit` bytes.
    fn decompress_block(&mut self, len: usize, limit: usize) -> Result<usize, DecompressError> {
        let input = &self.src[..len];
        let end = self.dst_start + limit;
        let frame_info = self.current_frame_info.unwrap_or_default();
        let content_len = self.content_len;
        let capacity = |end| dst_capacity(&frame_info, content_len, end);
        let linked = frame_info.block_mode == BlockMode::Linked;
        if linked && self.ext_dict_len != 0 {
            debug_assert!(end <= self.ext_dict_offset);
            let (head, tail) = self.dst.split_at_mut(self.ext_dict_offset);
            let ext_dict = &tail[..self.ext_dict_len];
            decompress_into_sink_with_dict::<true>(
                input,
                &mut SliceSink::new(&mut head[..end], self.dst_start),
                ext_dict,
            )
        } else if !self.dict.is_empty() {
            decompress_into_sink_with_dict::<true>(
                input,
                &mut vec_sink_for_decompression(&mut self.dst, self.dst_start, end, capacity),
                &self.dict,
            )
        } else {
            decompress_into_sink_with_dict::<false>(
                input,
                &mut vec_sink_for_decompression(&mut self.dst, self.dst_start, end, capacity),
                b"",
            )
        }
    }

    /// Decodes the frame's next block to the end of `buf`, as
    /// [`read_block`](Self::read_block) decodes it into `dst`, from where
    /// `read_to_end` would copy it. A linked block refers to the frame's
    /// earlier blocks, so it needs `frame_start`: where the frame starts in
    /// `buf`, when all of it was decoded there. A block that fails leaves
    /// `buf` as it was.
    fn read_block_into(
        &mut self,
        buf: &mut Vec<u8>,
        frame_start: Option<usize>,
    ) -> io::Result<usize> {
        debug_assert_eq!(self.dst_start, self.dst_end);
        let frame_info = *self
            .current_frame_info
            .as_ref()
            .ok_or_else(|| io::Error::other("no frame header has been read"))?;
        let pos = buf.len();
        let allowed = self.max_output.saturating_sub(self.bytes_output);
        let result = match self.read_block_info()? {
            BlockInfo::Uncompressed(len) => {
                let len = len as usize;
                if len > frame_info.block_size.get_size() {
                    return Err(Error::BlockTooBig.into());
                }
                let bytes_output = self.check_output_len(len)?;
                reserve_block(buf, len, &frame_info, self.content_len, allowed);
                buf.resize(pos + len, 0);
                self.r.read_exact(&mut buf[pos..]).and_then(|()| {
                    if frame_info.block_checksums {
                        let expected_checksum = Self::read_checksum(&mut self.r)?;
                        Self::check_block_checksum(&buf[pos..], expected_checksum)?;
                    }
                    Ok(bytes_output)
                })
            }
            BlockInfo::Compressed(len) => {
                let len = len as usize;
                self.read_compressed(&frame_info, len)?;
                self.decompress_block_into(&frame_info, len, buf, frame_start)
                    .map_err(|e| Error::DecompressionError(e).into())
                    .and_then(|()| self.check_output_len(buf.len() - pos))
            }
            BlockInfo::EndMark => {
                self.end_frame(&frame_info)?;
                return Ok(0);
            }
        };
        match result {
            Ok(bytes_output) => self.bytes_output = bytes_output,
            Err(e) => {
                buf.truncate(pos);
                return Err(e);
            }
        }
        let block = &buf[pos..];
        self.content_len += block.len() as u64;
        if frame_info.content_checksum {
            self.content_hasher.write(block);
        }
        Ok(block.len())
    }

    /// Decodes the compressed block in `src[..len]` to the end of `buf` (see
    /// [`read_block_into`](Self::read_block_into)), with the limits of
    /// [`read_block`](Self::read_block).
    fn decompress_block_into(
        &self,
        frame_info: &FrameInfo,
        len: usize,
        buf: &mut Vec<u8>,
        frame_start: Option<usize>,
    ) -> Result<(), DecompressError> {
        let max_block_size = frame_info.block_size.get_size();
        let pos = buf.len();
        let input = &self.src[..len];
        // The earlier output a block may refer to: the dictionary for an
        // independent block, and for a linked one the frame's last 64 KiB
        // and, before the frame, the dictionary.
        let (history, dict) = match frame_start {
            Some(start) if frame_info.block_mode == BlockMode::Linked => {
                let history = start.max(pos.saturating_sub(WINDOW_SIZE));
                (
                    history,
                    if history == start {
                        &self.dict[..]
                    } else {
                        &[]
                    },
                )
            }
            _ => (pos, &self.dict[..]),
        };
        let mut limit = self.first_limit(frame_info, len, 0);
        let allowed = self.max_output.saturating_sub(self.bytes_output);
        loop {
            reserve_block(buf, limit, frame_info, self.content_len, allowed);
            buf.resize(pos + limit, 0);
            let sink = &mut SliceSink::new(&mut buf[history..], pos - history);
            let result = if dict.is_empty() {
                decompress_into_sink_with_dict::<false>(input, sink, b"")
            } else {
                decompress_into_sink_with_dict::<true>(input, sink, dict)
            };
            match result {
                Err(_) if limit < max_block_size => limit = max_block_size,
                // Lengths from the block start, as from `dst`.
                Err(DecompressError::OutputTooSmall { expected, actual }) => {
                    let before = pos - history;
                    return Err(DecompressError::OutputTooSmall {
                        expected: expected.saturating_sub(before),
                        actual: actual.saturating_sub(before),
                    });
                }
                Err(e) => return Err(e),
                Ok(written) => {
                    buf.truncate(pos + written);
                    return Ok(());
                }
            }
        }
    }

    fn read_more(&mut self) -> io::Result<usize> {
        loop {
            if self.current_frame_info.is_none() && self.read_frame_info()? == 0 {
                return Ok(0);
            }
            let read = match self.read_block() {
                Ok(read) => read,
                Err(e) => {
                    // The frame's history, length, and checksum state are
                    // unusable after a failed block.
                    self.current_frame_info = None;
                    return Err(e);
                }
            };
            if read != 0 {
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
        let start = buf.len();
        // NOTE: with no declared content size, a block decodes within a
        // guessed limit (`first_limit`), so `buf` can keep up to 8x a
        // block's compressed length, at most the block size, in spare
        // capacity. Kept: shrinking it at the end made 1 MiB frames 3.7x
        // slower to decode (glibc trimmed and refaulted the heap each call).
        // Where the frame being decoded starts in `buf`, when its header was
        // read here and so all of its blocks were decoded there.
        let mut frame_start = None;
        loop {
            if self.dst_start < self.dst_end {
                // A block a `read` or `fill_buf` left.
                reserve_output(
                    buf,
                    self.dst_end - self.dst_start,
                    self.current_frame_info.and_then(|info| info.content_size),
                    self.content_len,
                    self.max_output.saturating_sub(self.bytes_output),
                );
                buf.extend_from_slice(&self.dst[self.dst_start..self.dst_end]);
                self.dst_start = self.dst_end;
            }
            // Blocks go straight to `buf`, except the linked blocks of a
            // frame that a `read` started: their history is in `dst`.
            let result = match self.current_frame_info {
                None => match self.read_frame_info() {
                    Ok(0) => return Ok(buf.len() - start),
                    Ok(_) => {
                        frame_start = Some(buf.len());
                        continue;
                    }
                    Err(e) => Err(e),
                },
                Some(info) if info.block_mode == BlockMode::Linked && frame_start.is_none() => {
                    self.read_block()
                }
                Some(_) => self.read_block_into(buf, frame_start),
            };
            if let Err(e) = result {
                // As in `read_more`: the frame's state is unusable.
                self.current_frame_info = None;
                if e.kind() != io::ErrorKind::Interrupted {
                    return Err(e);
                }
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
fn vec_resize_and_get_mut(
    v: &mut Vec<u8>,
    start: usize,
    end: usize,
    capacity: impl FnOnce(usize) -> usize,
) -> &mut [u8] {
    grow_zeroed(v, start, end, capacity);
    &mut v[start..end]
}
