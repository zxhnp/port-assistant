use crate::{Framing, MAX_FRAME, Packet};

struct Delimiter {
    bytes: Vec<u8>,
    fallback: Vec<usize>,
}

impl Delimiter {
    fn new(bytes: Vec<u8>) -> Self {
        let mut fallback = vec![0; bytes.len()];
        for index in 1..bytes.len() {
            fallback[index] = Self::advance(&bytes, &fallback, fallback[index - 1], bytes[index]);
        }
        Self { bytes, fallback }
    }

    fn advance(bytes: &[u8], fallback: &[usize], mut matched: usize, byte: u8) -> usize {
        while matched > 0 && bytes[matched] != byte {
            matched = fallback[matched - 1];
        }
        matched + usize::from(bytes[matched] == byte)
    }

    fn match_byte(&self, matched: usize, byte: u8) -> usize {
        Self::advance(&self.bytes, &self.fallback, matched, byte)
    }
}

pub(crate) struct Decoder {
    header: Delimiter,
    tail: Delimiter,
    buffered: Vec<u8>,
    header_match: usize,
    tail_match: usize,
    limit: usize,
}

impl Decoder {
    pub(crate) fn new(framing: &Framing) -> Self {
        Self {
            header: Delimiter::new(framing.header.clone()),
            tail: Delimiter::new(framing.tail.clone()),
            buffered: Vec::new(),
            header_match: 0,
            tail_match: 0,
            limit: MAX_FRAME,
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> (Vec<Packet>, bool) {
        let mut packets = Vec::new();
        let mut overflow = false;
        for &byte in bytes {
            // 保留头部的部分匹配，以处理跨读取或重叠的分隔字节。
            self.header_match = self.header.match_byte(self.header_match, byte);
            let has_header = self.header_match == self.header.bytes.len();
            if has_header {
                self.header_match = self.header.fallback[self.header_match - 1];
            }

            if self.buffered.is_empty() {
                if has_header {
                    self.buffered.extend(&self.header.bytes);
                }
                continue;
            }

            if self.buffered.len() == self.limit {
                self.buffered.clear();
                self.tail_match = 0;
                overflow = true;
                if has_header {
                    self.buffered.extend(&self.header.bytes);
                }
                continue;
            }

            self.buffered.push(byte);
            self.tail_match = self.tail.match_byte(self.tail_match, byte);
            if self.tail_match == self.tail.bytes.len() {
                packets.push(Packet {
                    bytes: std::mem::take(&mut self.buffered),
                    header_len: self.header.bytes.len(),
                    tail_len: self.tail.bytes.len(),
                });
                self.header_match = 0;
                self.tail_match = 0;
            }
        }
        (packets, overflow)
    }
}

pub(crate) fn raw_packet(bytes: Vec<u8>) -> Packet {
    Packet {
        bytes,
        header_len: 0,
        tail_len: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framing() -> Framing {
        Framing {
            header: vec![0xaa, 0x55],
            tail: vec![0x0d, 0x0a],
        }
    }

    #[test]
    fn finds_multiple_frames_across_reads() {
        let mut decoder = Decoder::new(&framing());
        assert!(decoder.feed(&[0x00, 0xaa]).0.is_empty());
        assert!(decoder.feed(&[0x55, 1, 0x0d]).0.is_empty());
        let (packets, overflow) = decoder.feed(&[0x0a, 0xaa, 0x55, 2, 0x0d, 0x0a, 0xaa]);
        assert!(!overflow);
        assert_eq!(packets[0].bytes, [0xaa, 0x55, 1, 0x0d, 0x0a]);
        assert_eq!(packets[1].bytes, [0xaa, 0x55, 2, 0x0d, 0x0a]);
        assert_eq!(decoder.feed(&[0x55, 0x0d, 0x0a]).0.len(), 1);
    }

    #[test]
    fn matches_overlapping_delimiter_prefixes() {
        let framing = Framing {
            header: b"aaab".to_vec(),
            tail: b"xxxy".to_vec(),
        };
        let mut decoder = Decoder::new(&framing);
        let (packets, _) = decoder.feed(b"aaaabxxxy");
        assert_eq!(packets[0].bytes, b"aaabxxxy");
    }

    #[test]
    fn oversized_incomplete_frame_resynchronizes() {
        let mut decoder = Decoder::new(&framing());
        decoder.limit = 6;
        let (packets, overflow) =
            decoder.feed(&[0xaa, 0x55, 1, 2, 3, 4, 0xaa, 0x55, 2, 0x0d, 0x0a]);
        assert!(overflow);
        assert_eq!(packets[0].bytes, [0xaa, 0x55, 2, 0x0d, 0x0a]);
    }
}
