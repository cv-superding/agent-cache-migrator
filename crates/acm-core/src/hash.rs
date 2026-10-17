//! 纯 Rust SHA-256（FIPS 180-4）—— 零依赖哲学下的 `--verify-hash` 基础。
//!
//! 为什么手写而不用 `sha2` crate：一个算法 ≈100 行 + 标准测试向量就能锁死正确性，
//! 换来整个项目零第三方运行时依赖的延续。标准向量（FIPS 180-4 示例 + 空串）在
//! 测试里逐位断言，安全性不靠自觉。
//!
//! 性能说明：校验要读两侧的每个文件，大目录本来就慢（这是它做成**可选项**的原因，
//! 见 `migrate::RunOptions::verify_hash`）。本实现按 64KB 块流式读取，
//! 不会把文件整个读进内存（对齐 OPTOPIA VerifyHash 的做法）。

const K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

const H0: [u32; 8] = [
    0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
];

/// 流式 SHA-256 计算器。
pub struct Sha256 {
    state: [u32; 8],
    buf: [u8; 64],
    buf_len: usize,
    /// 已消费的总字节数（不含 padding）
    total: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    pub fn new() -> Self {
        Self {
            state: H0,
            buf: [0; 64],
            buf_len: 0,
            total: 0,
        }
    }

    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        // 先把上次残留的半块填满
        if self.buf_len > 0 {
            let need = 64 - self.buf_len;
            let take = need.min(data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                self.process_block(&block);
                self.buf_len = 0;
            }
        }
        // 整块直接处理
        while data.len() >= 64 {
            let (block, rest) = data.split_at(64);
            self.process_block(block.try_into().unwrap());
            data = rest;
        }
        // 尾巴存进缓冲
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// 结束并输出 32 字节摘要。
    pub fn finish(mut self) -> [u8; 32] {
        let bit_len = self.total.wrapping_mul(8);
        // padding：0x80 + 0x00… + 8 字节大端位长
        self.update(&[0x80]);
        while self.buf_len != 56 {
            self.update(&[0]);
        }
        // 注意：上面 update 会污染 total —— 位长在 padding 前就算好了
        self.buf[56..64].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        self.process_block(&block);

        let mut out = [0u8; 32];
        for (i, w) in self.state.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }

    fn process_block(&mut self, block: &[u8; 64]) {
        let mut w = [0u32; 64];
        for (i, chunk) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*chunk);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut h = self.state;
        for i in 0..64 {
            let s1 = h[4].rotate_right(6) ^ h[4].rotate_right(11) ^ h[4].rotate_right(25);
            let ch = (h[4] & h[5]) ^ ((!h[4]) & h[6]);
            let t1 = h[7]
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = h[0].rotate_right(2) ^ h[0].rotate_right(13) ^ h[0].rotate_right(22);
            let maj = (h[0] & h[1]) ^ (h[0] & h[2]) ^ (h[1] & h[2]);
            let t2 = s0.wrapping_add(maj);

            h[7] = h[6];
            h[6] = h[5];
            h[5] = h[4];
            h[4] = h[3].wrapping_add(t1);
            h[3] = h[2];
            h[2] = h[1];
            h[1] = h[0];
            h[0] = t1.wrapping_add(t2);
        }
        for (s, v) in self.state.iter_mut().zip(h.iter()) {
            *s = s.wrapping_add(*v);
        }
    }
}

/// 便捷：字节数组的十六进制摘要。
pub fn sha256_hex(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    to_hex(&h.finish())
}

/// 流式计算一个文件的 SHA-256（64KB 块，不整读进内存）。
pub fn sha256_file(path: &Path) -> Result<[u8; 32], String> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).map_err(|e| format!("{}：{e}", path.display()))?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f
            .read(&mut buf)
            .map_err(|e| format!("{}：{e}", path.display()))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finish())
}

pub fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;

    /// FIPS 180-4 的标准测试向量 —— 算法正确性的锚点。
    #[test]
    fn fips_vectors() {
        // 空串
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // "abc"
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // 两块整 + 部分块
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    /// 分多次 update 与一次 update 结果必须一致（流式正确性）。
    #[test]
    fn streaming_equals_oneshot() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let mut h = Sha256::new();
        for chunk in data.chunks(997) {
            h.update(chunk);
        }
        assert_eq!(to_hex(&h.finish()), sha256_hex(&data));
    }

    /// 文件哈希与内存哈希一致。
    #[test]
    fn file_hash_matches_memory() {
        let p = std::env::temp_dir().join(format!("acm-sha-{}", std::process::id()));
        std::fs::write(&p, b"hello sha256").unwrap();
        let f = sha256_file(&p).unwrap();
        assert_eq!(to_hex(&f), sha256_hex(b"hello sha256"));
        let _ = std::fs::remove_file(&p);
    }
}
