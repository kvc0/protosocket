use std::{hint::black_box, io::IoSlice, mem::MaybeUninit};

use bytes::Buf;
use criterion::{criterion_main, Criterion};

fn vec_or_unsafe_array(criterion: &mut Criterion) {
    let mut group = criterion.benchmark_group("vec_or_unsafe_array");

    const UIO_MAXIOV: usize = 128;

    group.bench_function("vec", |bencher| {
        let source_material: Vec<Vec<u8>> = (0..200).map(|_| vec(200)).collect();
        bencher.iter(|| {
            let buffer: Vec<IoSlice<'_>> = source_material
                .iter()
                .take(UIO_MAXIOV)
                .map(|buffer| IoSlice::new(&buffer))
                .collect();
            black_box(buffer);
        });
    });

    group.bench_function("unsafe_array", |bencher| {
        let source_material: Vec<OwnedBuffer> = (0..200).map(|_| buf(200)).collect();

        bencher.iter(|| {
            const {
                assert!(!std::mem::needs_drop::<IoSlice>());
            }
            let mut buffers = unsafe {
                std::mem::transmute::<_, [IoSlice; UIO_MAXIOV]>(
                    [MaybeUninit::<IoSlice>::uninit(); UIO_MAXIOV],
                )
            };
            let mut iov = 0;
            let mut buffer_slice = buffers.as_mut_slice();
            for buffer in &source_material {
                let distance = buffer.chunks_vectored(buffer_slice);
                iov += distance;
                if iov == UIO_MAXIOV {
                    break;
                }
                buffer_slice = &mut buffer_slice[distance..];
            }
            let initialized_slice = &buffers[..iov];
            black_box(initialized_slice);
        });
    });
}

fn vec(len: usize) -> Vec<u8> {
    (0..len).map(|i| i as u8).collect()
}

struct OwnedBuffer {
    buffer: Vec<u8>,
    cursor: usize,
}
impl OwnedBuffer {
    pub fn new(buffer: Vec<u8>) -> Self {
        Self { buffer, cursor: 0 }
    }
}
impl bytes::Buf for OwnedBuffer {
    #[inline(always)]
    fn remaining(&self) -> usize {
        self.buffer.len() - self.cursor
    }

    #[inline(always)]
    fn chunk(&self) -> &[u8] {
        &self.buffer[self.cursor..]
    }

    #[inline(always)]
    fn advance(&mut self, cnt: usize) {
        assert!(self.buffer.len() <= self.cursor + cnt);
        self.cursor += cnt;
    }
}

fn buf(len: usize) -> OwnedBuffer {
    OwnedBuffer::new((0..len).map(|i| i as u8).collect())
}

criterion_main! {
    stack,
}
criterion::criterion_group!(stack, vec_or_unsafe_array);
