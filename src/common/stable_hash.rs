//! FNV-1a, 64 bits, as a [`Hasher`]. Fixed, unlike `DefaultHasher`, whose output may change with
//! the toolchain and would quietly turn every cached file into a miss.

use std::hash::Hasher;

pub struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 ^= *byte as u64;
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub fn of_text(text: &str) -> u64 {
    let mut hasher = Fnv::default();
    hasher.write(text.as_bytes());
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Breaks if the function changes, which would orphan every cached file.
    #[test]
    fn the_hash_is_the_same_on_every_build() {
        assert_eq!(of_text("1:grackle"), 0xb17f_e6b2_dd3c_1ffb);
    }
}
