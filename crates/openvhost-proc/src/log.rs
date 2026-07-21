// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded log storage (2,000 lines/service, drop-oldest) + level heuristic.

use std::collections::VecDeque;

use crate::events::{LogLevel, LogLine, StreamSource};

#[allow(dead_code)]
pub(crate) const RING_CAPACITY: usize = 2000;
#[allow(dead_code)]
pub(crate) const STDERR_TAIL: usize = 10;

pub(crate) struct RingBuffer {
    cap: usize,
    items: VecDeque<LogLine>,
}

impl RingBuffer {
    #[allow(dead_code)]
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            cap,
            items: VecDeque::with_capacity(cap.min(256)),
        }
    }
    #[allow(dead_code)]
    pub(crate) fn push(&mut self, line: LogLine) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(line);
    }
    #[allow(dead_code)]
    pub(crate) fn tail(&self, n: usize) -> Vec<LogLine> {
        self.items.iter().rev().take(n).rev().cloned().collect()
    }
    #[allow(dead_code)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// v0 heuristic (spec §3): "ERROR" anywhere → Error; else "WARN" → Warn;
/// else Info. Same rule for both streams.
#[allow(dead_code)]
pub(crate) fn classify_level(_source: StreamSource, line: &str) -> LogLevel {
    if line.contains("ERROR") {
        LogLevel::Error
    } else if line.contains("WARN") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn line(s: &str) -> LogLine {
        LogLine {
            ts_ms: 0,
            level: LogLevel::Info,
            line: s.to_string(),
        }
    }

    #[test]
    fn ring_drops_oldest_at_capacity() {
        let mut rb = RingBuffer::new(3);
        for i in 0..5 {
            rb.push(line(&format!("l{i}")));
        }
        assert_eq!(rb.len(), 3);
        let tail: Vec<String> = rb.tail(3).into_iter().map(|l| l.line).collect();
        assert_eq!(tail, vec!["l2", "l3", "l4"]);
    }

    #[test]
    fn tail_smaller_than_len() {
        let mut rb = RingBuffer::new(10);
        for i in 0..4 {
            rb.push(line(&format!("l{i}")));
        }
        let tail: Vec<String> = rb.tail(2).into_iter().map(|l| l.line).collect();
        assert_eq!(tail, vec!["l2", "l3"]);
    }

    #[test]
    fn level_heuristic() {
        assert_eq!(
            classify_level(StreamSource::Stderr, "ERROR boom"),
            LogLevel::Error
        );
        assert_eq!(
            classify_level(StreamSource::Stdout, "some WARN here"),
            LogLevel::Warn
        );
        assert_eq!(
            classify_level(StreamSource::Stderr, "hello"),
            LogLevel::Info
        );
    }
}
