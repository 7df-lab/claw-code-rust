const HEAD_LIMIT: usize = 512 * 1024;
const TAIL_LIMIT: usize = 512 * 1024;

pub struct HeadTailBuffer {
    head: Vec<u8>,
    tail: Vec<u8>,
    total: usize,
    dropped: bool,
    head_limit: usize,
    tail_limit: usize,
}

impl HeadTailBuffer {
    pub fn new() -> Self {
        HeadTailBuffer {
            head: Vec::new(),
            tail: Vec::new(),
            total: 0,
            dropped: false,
            head_limit: HEAD_LIMIT,
            tail_limit: TAIL_LIMIT,
        }
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.total += bytes.len();

        if self.head.len() < self.head_limit {
            let space = self.head_limit - self.head.len();
            let take = bytes.len().min(space);
            self.head.extend_from_slice(&bytes[..take]);

            if take < bytes.len() {
                self.dropped = true;
                let remaining = &bytes[take..];

                if remaining.len() > self.tail_limit {
                    self.tail = remaining[remaining.len() - self.tail_limit..].to_vec();
                } else {
                    self.tail.extend_from_slice(remaining);
                    if self.tail.len() > self.tail_limit {
                        let excess = self.tail.len() - self.tail_limit;
                        self.tail.drain(0..excess);
                    }
                }
            }
        } else {
            self.dropped = true;
            if bytes.len() > self.tail_limit {
                self.tail = bytes[bytes.len() - self.tail_limit..].to_vec();
            } else {
                self.tail.extend_from_slice(bytes);
                if self.tail.len() > self.tail_limit {
                    let excess = self.tail.len() - self.tail_limit;
                    self.tail.drain(0..excess);
                }
            }
        }
    }

    pub fn collect(&self) -> String {
        let mut result = String::with_capacity(self.head.len() + self.tail.len() + 100);

        // SAFETY: head bytes are from PTY output, lossy conversion is acceptable
        let head_str = String::from_utf8_lossy(&self.head);
        result.push_str(&head_str);

        if self.dropped {
            result.push_str("\n\n... [output truncated]\n\n");
        }

        let tail_str = String::from_utf8_lossy(&self.tail);
        result.push_str(&tail_str);

        result
    }

    /// Collect raw bytes (for when callers need Vec<u8> directly)
    pub fn collect_bytes(&self) -> Vec<u8> {
        let mut result = Vec::with_capacity(self.head.len() + self.tail.len() + 100);
        result.extend_from_slice(&self.head);
        if self.dropped {
            result.extend_from_slice(b"\n\n... [output truncated]\n\n");
        }
        result.extend_from_slice(&self.tail);
        result
    }

    pub fn total(&self) -> usize {
        self.total
    }

    pub fn truncated(&self) -> bool {
        self.dropped
    }
}

impl Default for HeadTailBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_keeps_small_content() {
        let mut buf = HeadTailBuffer::new();
        buf.push(b"hello world");
        let result = buf.collect();
        assert_eq!(result, "hello world");
        assert!(!buf.truncated());
    }

    #[test]
    fn buffer_keeps_head_and_tail_when_overflow() {
        let mut buf = HeadTailBuffer::new();
        buf.head_limit = 10;
        buf.tail_limit = 10;

        let data = b"0123456789ABCDEFGHIJ";
        buf.push(data);
        let result = buf.collect();
        assert!(result.starts_with("0123456789"));
        assert!(result.contains("GHIJ"));
        assert!(buf.truncated());
        assert_eq!(buf.total(), 20);
    }

    #[test]
    fn buffer_preserves_tail_across_multiple_pushes() {
        let mut buf = HeadTailBuffer::new();
        buf.head_limit = 5;
        buf.tail_limit = 5;

        buf.push(b"AAAAA");
        buf.push(b"BBBBB");
        buf.push(b"CCCCC");
        let result = buf.collect();
        assert!(result.starts_with("AAAAA"));
        assert!(result.contains("CCCCC"));
        assert!(buf.truncated());
    }

    #[test]
    fn empty_buffer_returns_empty() {
        let buf = HeadTailBuffer::new();
        assert!(buf.collect().is_empty());
    }
}
