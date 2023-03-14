// This code is based on the Soci Snapshotter which was based on zlib, but only
// includes the needed pieces for building ztocs and is written in Rust instead
// of C.

/*
   Copyright The Soci Snapshotter Authors.

   Licensed under the Apache License, Version 2.0 (the "License");
   you may not use this file except in compliance with the License.
   You may obtain a copy of the License at

       http://www.apache.org/licenses/LICENSE-2.0

   Unless required by applicable law or agreed to in writing, software
   distributed under the License is distributed on an "AS IS" BASIS,
   WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
   See the License for the specific language governing permissions and
   limitations under the License.
*/

/*
  Copyright (C) 1995-2017 Jean-loup Gailly and Mark Adler
  This software is provided 'as-is', without any express or implied
  warranty.  In no event will the authors be held liable for any damages
  arising from the use of this software.
  Permission is granted to anyone to use this software for any purpose,
  including commercial applications, and to alter it and redistribute it
  freely, subject to the following restrictions:
  1. The origin of this software must not be misrepresented; you must not
     claim that you wrote the original software. If you use this software
     in a product, an acknowledgment in the product documentation would be
     appreciated but is not required.
  2. Altered source versions must be plainly marked as such, and must not be
     misrepresented as being the original software.
  3. This notice may not be removed or altered from any source distribution.
  Jean-loup Gailly        Mark Adler
  jloup@gzip.org          madler@alumni.caltech.edu
*/

use std::{
    alloc::{self, Layout},
    cmp,
    ffi::{CStr, CString},
    io::{self, Read, Result},
    mem, ptr,
};

use libc::{c_int, c_void};
use libz_sys::{
    inflate, inflateInit2_, uInt, z_stream, zlibVersion, Z_BLOCK, Z_BUF_ERROR, Z_DATA_ERROR,
    Z_MEM_ERROR, Z_NEED_DICT, Z_STREAM_END, Z_STREAM_ERROR, Z_VERSION_ERROR,
};

// Since gzip is compressed with 32 KiB window size, WINDOW_SIZE is fixed
const WINSIZE: usize = 32768;
const CHUNK: usize = 1 << 14;

/// A checkpoint includes information about the current state of the decompressor at specific
/// locations in the compressed payload. Decompression can be resumed at any checkpoint, using the
/// context stored in the checkpoint, without requiring decompressing the rest of the payload.
#[derive(PartialEq, Eq)]
pub struct GZipCheckpoint {
    pub out: usize,
    pub r#in: usize,
    pub bits: u8,
    pub window: [u8; WINSIZE],
}

impl std::fmt::Debug for GZipCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GzipCheckout")
            .field("out", &self.out)
            .field("in", &self.r#in)
            .field("bits", &format_args!("0b{:08b}", self.bits))
            .finish()
    }
}

/// Information about the compressed payload. Includes checkpoints which allow for quickly
/// decompressing subets of the compressed payload.
#[derive(Debug, PartialEq, Eq)]
pub struct ZInfo {
    pub version: i32,
    pub checkpoints: Vec<GZipCheckpoint>,
    pub span_size: usize,
    pub total_in: usize,
    pub total_out: usize,
}

/// A wrapper around the underlying [`z_stream`].
struct ZStream {
    stream: Box<z_stream>,
}

impl ZStream {
    /// Initializes a new ZStream used for inflating.
    fn new(window_bits: c_int) -> Result<Self> {
        let mut stream = Box::new(z_stream {
            next_in: ptr::null_mut(),
            avail_in: 0,
            total_in: 0,
            next_out: ptr::null_mut(),
            avail_out: 0,
            total_out: 0,
            msg: ptr::null_mut(),
            state: ptr::null_mut(),
            opaque: ptr::null_mut(),
            data_type: 0,
            adler: 0,
            reserved: 0,
            zalloc,
            zfree,
        });
        check_error(
            unsafe {
                inflateInit2_(
                    stream.as_mut() as *mut z_stream,
                    window_bits,
                    zlibVersion(),
                    mem::size_of::<z_stream>() as c_int,
                )
            },
            None,
        )?;

        Ok(Self { stream })
    }

    /// Returns the amount of bytes available for the stream to read from the input buffer.
    fn available_in(&self) -> u32 {
        self.stream.avail_in
    }

    /// Returns the amount of bytes available for the stream to write to in the output buffer.
    fn available_out(&self) -> u32 {
        self.stream.avail_out
    }

    /// Returns the current data type of the stream.
    fn data_type(&self) -> i32 {
        self.stream.data_type
    }

    /// Sets the input buffer that the stream will read from.
    // TODO: This is really sketchy, we are not following ownership rules properly...
    unsafe fn next_in(&mut self, r#in: &mut [u8]) {
        self.stream.avail_in = r#in.len() as u32;
        self.stream.next_in = r#in.as_mut_ptr() as *mut u8;
    }

    /// Sets the output butter that the stream will write to.
    // TODO: This is really sketchy, we are not following ownership rules properly...
    unsafe fn next_out(&mut self, out: &mut [u8]) {
        self.stream.avail_out = out.len() as u32;
        self.stream.next_out = out.as_mut_ptr() as *mut u8;
    }

    /// Inflates the next part of the stream. Input will be read from the input buffer and output
    /// will be placed into the output buffer.
    fn inflate(&mut self, flush: c_int) -> Result<c_int> {
        check_error(
            unsafe { inflate(self.stream.as_mut() as *mut z_stream, flush) },
            Some(&self.stream),
        )
    }
}

impl Drop for ZStream {
    fn drop(&mut self) {
        unsafe {
            libz_sys::inflateEnd(self.stream.as_mut() as *mut z_stream);
        }
    }
}

/// A helper to convert zlib errors into [`io::Error`]s.
fn check_error(ret: c_int, stream: Option<&z_stream>) -> Result<c_int> {
    let msg = stream.and_then(|stream| {
        if !stream.msg.is_null() {
            Some(unsafe { CStr::from_ptr(stream.msg).to_string_lossy().to_string() })
        } else {
            None
        }
    });
    match ret {
        Z_STREAM_ERROR => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib stream error".into()),
        )),
        Z_DATA_ERROR => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib data error".into()),
        )),
        Z_MEM_ERROR => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib mem error".into()),
        )),
        Z_BUF_ERROR => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib buf error".into()),
        )),
        Z_VERSION_ERROR => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib version error".into()),
        )),
        ret if ret < 0 => Err(io::Error::new(
            io::ErrorKind::Other,
            msg.unwrap_or_else(|| "zlib unknown error".into()),
        )),
        ret => Ok(ret),
    }
}

/// A Gzip decompressor that also generates compression metadata which can be used to read
/// parts of the compressed payload without needing to decompress everything.
pub struct GzipZInfoDecompressor<R> {
    reader: R,

    stream: ZStream,
    zinfo: ZInfo,

    window: RingBuffer<u8, WINSIZE>,
    input: [u8; CHUNK],
    last_block: usize,
}

impl<R> GzipZInfoDecompressor<R>
where
    R: Read,
{
    /// Creates a new Gzip zinfo Decompressor. The span size specifies the minimum size of a span
    /// recording in the zinfo.
    pub fn new(reader: R, span_size: usize) -> Result<Self> {
        let stream = ZStream::new(47)?;
        let zinfo = ZInfo {
            version: 2,
            checkpoints: Vec::new(),
            span_size,
            total_in: 0,
            total_out: 0,
        };

        Ok(Self {
            reader,
            stream,
            zinfo,
            window: RingBuffer::new(),
            input: [0u8; CHUNK],
            last_block: 0,
        })
    }

    /// Consumes the decompressor to return the zinfo compression metadata. The index is only complete
    /// once EOF is reached.
    pub fn into_zinfo(self) -> ZInfo {
        self.zinfo
    }
}

impl<R> Read for GzipZInfoDecompressor<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        unsafe {
            self.stream.next_out(buf);
        }
        let mut read = 0;

        while self.stream.available_out() > 0 {
            if self.stream.available_in() == 0 {
                let count = self.reader.read(&mut self.input)?;
                unsafe {
                    self.stream.next_in(&mut self.input[..count]);
                }
            }

            let last_read = read;
            self.zinfo.total_in += self.stream.available_in() as usize;
            self.zinfo.total_out += self.stream.available_out() as usize;
            read += self.stream.available_out() as usize;
            let status = self.stream.inflate(Z_BLOCK)?;
            self.zinfo.total_in -= self.stream.available_in() as usize;
            self.zinfo.total_out -= self.stream.available_out() as usize;
            read -= self.stream.available_out() as usize;
            if status == Z_NEED_DICT {
                return Err(io::Error::new(io::ErrorKind::Other, "unexpected need dict"));
            }
            if status == Z_STREAM_END {
                return Ok(read);
            }

            // Copy the read data into the sliding window.
            self.window
                .write(&buf[last_read..buf.len() - self.stream.available_out() as usize]);

            if (self.stream.data_type() & 128) != 0
                && (self.stream.data_type() & 64) == 0
                && (self.zinfo.total_out == 0
                    || self.zinfo.total_out - self.last_block > self.zinfo.span_size)
            {
                let mut checkpoint = GZipCheckpoint {
                    bits: (self.stream.data_type() as u8) & 7,
                    r#in: self.zinfo.total_in,
                    out: self.zinfo.total_out,
                    window: [0u8; WINSIZE],
                };
                let (left, right) = self.window.read();
                checkpoint.window[..left.len()].copy_from_slice(left);
                checkpoint.window[left.len()..].copy_from_slice(right);
                self.zinfo.checkpoints.push(checkpoint);
                self.last_block = self.zinfo.total_out;
            }
        }

        Ok(read)
    }
}

/// A fixed-size ring buffer. Writes are pushed onto the back of the buffer.
struct RingBuffer<T, const N: usize> {
    buffer: [T; N],
    index: usize,
}

impl<T, const N: usize> RingBuffer<T, N>
where
    T: Copy + Default,
{
    /// Creates a new ring buffer.
    fn new() -> Self {
        Self {
            buffer: [T::default(); N],
            index: 0,
        }
    }

    /// Writes the buffer to the back of the ring buffer.
    fn write(&mut self, mut buf: &[T]) {
        if buf.is_empty() {
            return;
        }

        if buf.len() > self.buffer.len() {
            buf = &buf[buf.len() - self.buffer.len()..];
        }

        while !buf.is_empty() {
            let size = cmp::min(buf.len(), self.buffer.len() - self.index);
            self.buffer[self.index..self.index + size].copy_from_slice(&buf[..size]);
            buf = &buf[size..];
            self.index = (self.index + size) % self.buffer.len();
        }
    }

    /// Gets the contents of the ring buffer. The underlying storage may be non-contiguous, so
    /// two slices are returned instead. The left slice is the front and the right slice is the
    /// back.
    fn read(&self) -> (&[T], &[T]) {
        (&self.buffer[self.index..], &self.buffer[..self.index])
    }
}

const ALIGN: usize = std::mem::align_of::<usize>();
type AllocSize = uInt;

fn align_up(size: usize, align: usize) -> usize {
    (size + align - 1) & !(align - 1)
}

extern "C" fn zalloc(_ptr: *mut c_void, items: AllocSize, item_size: AllocSize) -> *mut c_void {
    // We need to multiply `items` and `item_size` to get the actual desired
    // allocation size. Since `zfree` doesn't receive a size argument we
    // also need to allocate space for a `usize` as a header so we can store
    // how large the allocation is to deallocate later.
    let size = match items
        .checked_mul(item_size)
        .and_then(|i| usize::try_from(i).ok())
        .map(|size| align_up(size, ALIGN))
        .and_then(|i| i.checked_add(std::mem::size_of::<usize>()))
    {
        Some(i) => i,
        None => return ptr::null_mut(),
    };

    // Make sure the `size` isn't too big to fail `Layout`'s restrictions
    let layout = match Layout::from_size_align(size, ALIGN) {
        Ok(layout) => layout,
        Err(_) => return ptr::null_mut(),
    };

    unsafe {
        // Allocate the data, and if successful store the size we allocated
        // at the beginning and then return an offset pointer.
        let ptr = alloc::alloc(layout) as *mut usize;
        if ptr.is_null() {
            return ptr as *mut c_void;
        }
        *ptr = size;
        ptr.add(1) as *mut c_void
    }
}

extern "C" fn zfree(_ptr: *mut c_void, address: *mut c_void) {
    unsafe {
        // Move our address being freed back one pointer, read the size we
        // stored in `zalloc`, and then free it using the standard Rust
        // allocator.
        let ptr = (address as *mut usize).offset(-1);
        let size = *ptr;
        let layout = Layout::from_size_align_unchecked(size, ALIGN);
        alloc::dealloc(ptr as *mut u8, layout)
    }
}

#[cfg(test)]
mod test {
    use std::io::Cursor;

    use super::*;

    #[test]
    fn test_generate_zinfo() {
        let mut reader = Cursor::new(include_bytes!("testdata/test.tar.gz"));
        let mut decoder = GzipZInfoDecompressor::new(&mut reader, 4096).unwrap();
        let mut buf = [0u8; 1 << 14];
        while decoder.read(&mut buf).unwrap() > 0 {}
        // TODO: Test with a larger tarball and add assertions on the zinfo index.
        let _new_info = decoder.into_zinfo();
    }

    #[test]
    fn test_ring_buffer() {
        let mut buffer = RingBuffer::<u8, 100>::new();

        assert_eq!(buffer.read(), ([0u8; 100].as_slice(), [0u8; 0].as_slice()));

        buffer.write(&[1u8; 50]);
        assert_eq!(buffer.read(), ([0u8; 50].as_slice(), [1u8; 50].as_slice()));

        buffer.write(&[2u8; 50]);
        let mut expected = Vec::new();
        expected.extend_from_slice(&[1u8; 50]);
        expected.extend_from_slice(&[2u8; 50]);
        assert_eq!(buffer.read(), (expected.as_slice(), [0u8; 0].as_slice()));

        buffer.write(&[3u8; 150]);
        assert_eq!(buffer.read(), ([3u8; 100].as_slice(), [0u8; 0].as_slice()));

        buffer.write(&[4u8; 75]);
        assert_eq!(buffer.read(), ([3u8; 25].as_slice(), [4u8; 75].as_slice()));
    }
}
