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

#[derive(PartialEq, Eq)]
pub struct GzipCheckpoint {
    pub out: usize,
    pub r#in: usize,
    pub bits: u8,
    pub window: [u8; WINSIZE],
}

impl std::fmt::Debug for GzipCheckpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GzipCheckout")
            .field("out", &self.out)
            .field("in", &self.r#in)
            .field("bits", &format_args!("0b{:08b}", self.bits))
            .finish()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct GzipZinfo {
    pub version: i32,
    pub checkpoints: Vec<GzipCheckpoint>,
    pub span_size: usize,
}

struct ZStream {
    stream: Box<z_stream>,
}

impl ZStream {
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
        check_error(unsafe {
            inflateInit2_(
                stream.as_mut() as *mut z_stream,
                window_bits,
                zlibVersion(),
                mem::size_of::<z_stream>() as c_int,
            )
        })?;

        Ok(Self { stream })
    }

    fn available_in(&self) -> u32 {
        self.stream.avail_in
    }

    fn available_out(&self) -> u32 {
        self.stream.avail_out
    }

    fn data_type(&self) -> i32 {
        self.stream.data_type
    }

    // TODO: This is really sketchy...
    fn next_in(&mut self, r#in: &mut [u8]) {
        self.stream.avail_in = r#in.len() as u32;
        self.stream.next_in = r#in.as_mut_ptr() as *mut u8;
    }

    // TODO: This is really sketchy...
    fn next_out(&mut self, out: &mut [u8]) {
        self.stream.avail_out = out.len() as u32;
        self.stream.next_out = out.as_mut_ptr() as *mut u8;
    }

    fn inflate(&mut self, flush: c_int) -> Result<c_int> {
        check_error(unsafe { inflate(self.stream.as_mut() as *mut z_stream, flush) })
    }
}

impl Drop for ZStream {
    fn drop(&mut self) {
        unsafe {
            libz_sys::inflateEnd(self.stream.as_mut() as *mut z_stream);
        }
    }
}

fn check_error(ret: c_int) -> Result<c_int> {
    match ret {
        Z_STREAM_ERROR => Err(io::Error::new(io::ErrorKind::Other, "zlib stream error")),
        Z_DATA_ERROR => Err(io::Error::new(io::ErrorKind::Other, "zlib data error")),
        Z_MEM_ERROR => Err(io::Error::new(io::ErrorKind::Other, "zlib memory error")),
        Z_BUF_ERROR => Err(io::Error::new(io::ErrorKind::Other, "zlib buf error")),
        Z_VERSION_ERROR => Err(io::Error::new(io::ErrorKind::Other, "zlib version error")),
        ret if ret < 0 => Err(io::Error::new(io::ErrorKind::Other, "unknown zlib error")),
        ret => Ok(ret),
    }
}

pub fn generate_zinfo<R: Read>(reader: &mut R, span_size: usize) -> Result<GzipZinfo> {
    // window is a ring buffer storing the last WINSIZE output.
    let mut window = [0u8; WINSIZE];
    let mut input = [0u8; CHUNK];
    let mut stream = ZStream::new(47)?;
    let mut total_in: usize = 0;
    let mut total_out: usize = 0;
    let mut last: usize = 0;

    let mut checkpoints = Vec::new();

    'OUTER: loop {
        let read = reader.read(&mut input)?;
        if read == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "unexpected EOF when reading zstream input",
            ));
        }
        stream.next_in(&mut input[..read]);

        loop {
            // Wrap back to the front of the window.
            if stream.available_out() == 0 {
                stream.next_out(&mut window);
            }

            total_in += stream.available_in() as usize;
            total_out += stream.available_out() as usize;
            let ret = stream.inflate(Z_BLOCK)?;
            total_in -= stream.available_in() as usize;
            total_out -= stream.available_out() as usize;
            if ret == Z_NEED_DICT {
                return Err(io::Error::new(io::ErrorKind::Other, "unexpected need dict"));
            }
            if ret == Z_STREAM_END {
                break 'OUTER;
            }

            if (stream.data_type() & 128) != 0
                && (stream.data_type() & 64) == 0
                && (total_out == 0 || total_out - last > span_size)
            {
                let mut checkpoint = GzipCheckpoint {
                    bits: (stream.data_type() as u8) & 7,
                    r#in: total_in,
                    out: total_out,
                    window: [0u8; WINSIZE],
                };
                // Copy the end of the window.
                if stream.available_out() > 0 {
                    // Here, we need to copy the back end of the window to the front of the
                    // checkpoint.
                    checkpoint.window[..stream.available_out() as usize]
                        .copy_from_slice(&window[(WINSIZE - stream.available_out() as usize)..]);
                }
                // Copy the front of the window.
                if (stream.available_out() as usize) < WINSIZE {
                    // Here, we need to copy the front end of the window to the back of the
                    // checkpoint.
                    checkpoint.window[stream.available_out() as usize..]
                        .copy_from_slice(&window[..(WINSIZE - stream.available_out() as usize)]);
                }
                checkpoints.push(checkpoint);
                last = total_out;
            }

            if stream.available_in() == 0 {
                break;
            }
        }
    }

    dbg!(total_out);
    dbg!(total_in);

    Ok(GzipZinfo {
        version: 2,
        checkpoints,
        span_size,
    })
}

/// A Gzip decompressor that also generates a
pub struct GzipZInfoDecompressor<R> {
    reader: R,

    stream: ZStream,
    zinfo: GzipZinfo,

    total_in: usize,
    total_out: usize,
    window: RingBuffer<u8, WINSIZE>,
    input: [u8; CHUNK],
    read_index: usize,
    last_block: usize,
}

impl<R> GzipZInfoDecompressor<R>
where
    R: Read,
{
    pub fn new(reader: R, span_size: usize) -> Result<Self> {
        let stream = ZStream::new(47)?;
        let zinfo = GzipZinfo {
            version: 2,
            checkpoints: Vec::new(),
            span_size,
        };

        Ok(Self {
            reader,
            stream,
            zinfo,
            total_in: 0,
            total_out: 0,
            window: RingBuffer::new(),
            input: [0u8; CHUNK],
            read_index: 0,
            last_block: 0,
        })
    }

    /// Returns the gzip index computed so far. The index is complete
    /// once EOF is reached.
    pub fn to_zinfo(self) -> GzipZinfo {
        self.zinfo
    }
}

impl<R> Read for GzipZInfoDecompressor<R>
where
    R: Read,
{
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.stream.next_out(buf);
        let mut read = 0;

        while self.stream.available_out() > 0 {
            if self.stream.available_in() == 0 {
                let count = self.reader.read(&mut self.input)?;
                self.stream.next_in(&mut self.input[..count]);
            }

            self.total_in += self.stream.available_in() as usize;
            self.total_out += self.stream.available_out() as usize;
            read += self.stream.available_out() as usize;
            let status = self.stream.inflate(Z_BLOCK)?;
            self.total_in -= self.stream.available_in() as usize;
            self.total_out -= self.stream.available_out() as usize;
            read -= self.stream.available_out() as usize;

            if status == Z_NEED_DICT {
                return Err(io::Error::new(io::ErrorKind::Other, "unexpected need dict"));
            }
            if status == Z_STREAM_END {
                return Ok(read);
            }

            if (self.stream.data_type() & 128) != 0
                && (self.stream.data_type() & 64) == 0
                && (self.total_out == 0 || self.total_out - self.last_block > self.zinfo.span_size)
            {
                let mut checkpoint = GzipCheckpoint {
                    bits: (self.stream.data_type() as u8) & 7,
                    r#in: self.total_in,
                    out: self.total_out,
                    window: [0u8; WINSIZE],
                };
                let (left, right) = self.window.read();
                checkpoint.window[..left.len()].copy_from_slice(&left);
                checkpoint.window[left.len()..].copy_from_slice(&right);
                self.zinfo.checkpoints.push(checkpoint);
                self.last_block = self.total_out;
            }
        }

        Ok(read)
    }
}

struct RingBuffer<T, const N: usize> {
    buffer: [T; N],
    index: usize,
}

impl<T, const N: usize> RingBuffer<T, N>
where
    T: Copy + Default,
{
    fn new() -> Self {
        Self {
            buffer: [T::default(); N],
            index: 0,
        }
    }

    fn write(&mut self, mut buf: &[T]) {
        if buf.len() == 0 {
            return;
        }

        if buf.len() > self.buffer.len() {
            buf = &buf[buf.len() - self.buffer.len()..];
        }

        while buf.len() > 0 {
            let size = cmp::min(buf.len(), self.buffer.len() - self.index);
            self.buffer[self.index..self.index + size].copy_from_slice(&buf[..size]);
            buf = &buf[size..];
            self.index = (self.index + size) % self.buffer.len();
        }
    }

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
        let old_info = generate_zinfo(&mut reader, 4096).expect("failed to generate zinfo");
        // TODO: Test with a larger tarball and add assertions on the zinfo index.
        let mut reader = Cursor::new(include_bytes!("testdata/test.tar.gz"));
        let mut decoder = GzipZInfoDecompressor::new(&mut reader, 4096).unwrap();
        let mut buf = [0u8; 1 << 14];
        while decoder.read(&mut buf).unwrap() > 0 {}
        let new_info = decoder.to_zinfo();
        assert_eq!(old_info, new_info);
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
