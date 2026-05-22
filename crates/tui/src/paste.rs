use std::collections::HashMap;

pub struct PasteStore {
    markers: HashMap<usize, String>,
    next_id: usize,
}

impl PasteStore {
    pub fn new() -> Self {
        Self {
            markers: HashMap::new(),
            next_id: 0,
        }
    }
    pub fn store(&mut self, content: &str) -> String {
        let line_count = content.lines().count();
        if line_count <= 10 {
            return content.trim_end_matches('\n').to_string();
        }
        let id = self.next_id;
        self.next_id += 1;
        self.markers.insert(id, content.to_string());
        format!("[paste #{} +{} lines]", id, line_count)
    }
    pub fn expand(&self, input: &str) -> String {
        let mut result = input.to_string();
        for (id, content) in &self.markers {
            let marker = format!("[paste #{} +{} lines]", id, content.lines().count());
            result = result.replace(&marker, content);
        }
        result
    }

    pub fn insert_if_large(&mut self, content: String) -> Option<usize> {
        if content.lines().count() > 10 {
            let id = self.next_id;
            self.markers.insert(id, content);
            self.next_id += 1;
            Some(id)
        } else {
            None
        }
    }

    pub fn get(&self, id: usize) -> Option<&str> {
        self.markers.get(&id).map(|s| s.as_str())
    }

    pub fn resolve_markers(&self, text: &str) -> String {
        let mut result = text.to_string();
        for (&id, content) in &self.markers {
            let start_pattern = format!("[paste #{}", id);
            while let Some(pos) = result.find(&start_pattern) {
                let end = result[pos..].find(']').map(|e| pos + e + 1);
                if let Some(end) = end {
                    result.replace_range(pos..end, content);
                } else {
                    break;
                }
            }
        }
        result
    }
}

impl Default for PasteStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_small_paste() {
        let mut s = PasteStore::new();
        assert_eq!(s.store("hi"), "hi");
    }
    #[test]
    fn test_large_paste_marker() {
        let mut s = PasteStore::new();
        let c = "line\n".repeat(15);
        let r = s.store(&c);
        assert!(r.contains("[paste #"));
    }
    #[test]
    fn test_expand_resolves() {
        let mut s = PasteStore::new();
        let c = "line\n".repeat(15);
        let m = s.store(&c);
        let expanded = s.expand(&format!("before {} after", m));
        assert!(expanded.starts_with("before line"));
    }
    #[test]
    fn test_expand_unknown_marker() {
        assert_eq!(
            PasteStore::new().expand("text [paste #99 +5 lines]"),
            "text [paste #99 +5 lines]"
        );
    }
}
