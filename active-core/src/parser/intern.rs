//! Per-evaluation include interning (L-P32).
//!
//! Keyed by content, never by a trust decision: the cache only reuses
//! bytes already read and token streams already lexed for the same
//! content hash within one parse/evaluation. Path resolution still runs
//! per include (a deleted path is E017 before this cache is consulted),
//! and it does not bound a fan-out of *distinct* files - the C-002 caps
//! still apply to every resolution.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::lexer::Lexed;

#[derive(Default)]
pub(crate) struct IncludeIntern {
    sources: HashMap<PathBuf, Arc<String>>,
    lexed: HashMap<String, Arc<Lexed>>,
    cached_tokens: usize,
    published: std::collections::HashSet<String>,
    #[cfg(test)]
    source_reads: usize,
    #[cfg(test)]
    lexes: usize,
}

/// Cache admission: a parse may include at most 256 files (C-002), and
/// the cache must not retain hundreds of megabytes of tokens for a
/// newline-dense graph. Streams over these bounds are parsed again
/// instead of cached; correctness is unaffected.
const MAX_CACHED_STREAMS: usize = 256;
const MAX_CACHED_TOKENS: usize = 200_000;
const MAX_STREAM_TOKENS: usize = 65_536;

impl IncludeIntern {
    /// Read a file once per parse; a repeated include reuses its bytes.
    pub(crate) fn source(&mut self, path: &Path) -> std::io::Result<Arc<String>> {
        if let Some(cached) = self.sources.get(path) {
            return Ok(cached.clone());
        }
        let text = Arc::new(std::fs::read_to_string(path)?);
        self.sources.insert(path.to_path_buf(), text.clone());
        #[cfg(test)]
        {
            self.source_reads += 1;
        }
        Ok(text)
    }

    /// A previously lexed stream for this content hash, if any.
    pub(crate) fn lexed(&self, hash: &str) -> Option<Arc<Lexed>> {
        self.lexed.get(hash).cloned()
    }

    /// Record one lexed stream. Only complete streams within the cache
    /// admission bounds are stored; a skipped stream is simply parsed
    /// again on a later include.
    pub(crate) fn store_lexed(&mut self, hash: String, lexed: Arc<Lexed>) {
        if lexed.budget_exceeded {
            return;
        }
        let tokens = lexed.tokens.len();
        if tokens > MAX_STREAM_TOKENS
            || self.lexed.len() >= MAX_CACHED_STREAMS
            || self.cached_tokens.saturating_add(tokens) > MAX_CACHED_TOKENS
        {
            return;
        }
        self.cached_tokens = self.cached_tokens.saturating_add(tokens);
        #[cfg(test)]
        {
            self.lexes += 1;
        }
        self.lexed.insert(hash, lexed);
    }

    /// Whether the content store already published this digest in this
    /// evaluation (L-P04).
    pub(crate) fn published(&self, hash: &str) -> bool {
        self.published.contains(hash)
    }

    /// Remember a successful store publish so a diamond does not re-read
    /// and re-compare the entry for every resolution.
    pub(crate) fn mark_published(&mut self, hash: String) {
        self.published.insert(hash);
    }

    #[cfg(test)]
    pub(crate) fn stats(&self) -> (usize, usize) {
        (self.source_reads, self.lexes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_source_is_read_once_per_parse() {
        let dir = std::env::temp_dir().join(format!(
            "active-intern-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("lib.active");
        std::fs::write(&path, "v = 1\n").expect("write");
        let mut intern = IncludeIntern::default();
        assert_eq!(&*intern.source(&path).expect("first"), "v = 1\n");
        assert_eq!(&*intern.source(&path).expect("second"), "v = 1\n");
        assert_eq!(intern.stats(), (1, 0), "one read, no lex yet");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lexed_streams_are_keyed_by_content_hash() {
        let empty = Arc::new(Lexed {
            tokens: Vec::new(),
            repairs: Vec::new(),
            errors: Vec::new(),
            comments: Vec::new(),
            budget_exceeded: false,
            errors_truncated: false,
        });
        let mut intern = IncludeIntern::default();
        assert!(intern.lexed("aa").is_none());
        intern.store_lexed("aa".to_string(), empty);
        assert!(intern.lexed("aa").is_some());
        assert!(intern.lexed("bb").is_none());
        assert_eq!(intern.stats(), (0, 1));
    }
}

#[cfg(test)]
mod extra_tests {
    use super::*;

    #[test]
    fn a_truncated_stream_is_not_cached() {
        let truncated = Arc::new(Lexed {
            tokens: Vec::new(),
            repairs: Vec::new(),
            errors: Vec::new(),
            comments: Vec::new(),
            budget_exceeded: true,
            errors_truncated: false,
        });
        let mut intern = IncludeIntern::default();
        intern.store_lexed("aa".to_string(), truncated);
        assert!(intern.lexed("aa").is_none());
        assert_eq!(intern.stats(), (0, 0));
    }

    #[test]
    fn the_source_cache_holds_one_snapshot() {
        // Helper-level contract: once read, the bytes stay available for
        // this parse. Path resolution (and E017 on a vanished file) is
        // the parser's job before it consults this cache.
        let dir = std::env::temp_dir().join(format!(
            "active-intern-snapshot-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("lib.active");
        std::fs::write(&path, "v = 1\n").expect("write");
        let mut intern = IncludeIntern::default();
        assert_eq!(&*intern.source(&path).expect("first"), "v = 1\n");
        std::fs::remove_file(&path).expect("delete");
        assert_eq!(
            &*intern.source(&path).expect("cached snapshot"),
            "v = 1\n",
            "one parse sees one snapshot of a path"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
}
