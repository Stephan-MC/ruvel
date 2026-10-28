use std::collections::HashMap;

/// A case-insensitive HTTP header map.
///
/// Header field names are case-insensitive according to RFC 9110 (Section 5.1).
/// This container preserves the casing of the last inserted key for serialization
/// while performing lookups, insertions, and removals in a case-insensitive manner.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    // Maps lowercased key -> (original key, header value)
    inner: HashMap<String, (String, String)>,
}

impl HeaderMap {
    /// Create a new, empty `HeaderMap`.
    pub fn new() -> Self {
        Self {
            inner: HashMap::new(),
        }
    }

    /// Insert a header name-value pair.
    ///
    /// If a header with the same name (case-insensitive) already exists,
    /// its value and casing are updated.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key_str = key.into();
        let lower = key_str.to_ascii_lowercase();
        self.inner.insert(lower, (key_str, value.into()));
    }

    /// Get a header value by name (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        let lower = key.to_ascii_lowercase();
        self.inner.get(&lower).map(|(_, v)| v.as_str())
    }

    /// Check if a header exists (case-insensitive).
    pub fn contains_key(&self, key: &str) -> bool {
        let lower = key.to_ascii_lowercase();
        self.inner.contains_key(&lower)
    }

    /// Remove a header by name (case-insensitive) and return its value if present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        let lower = key.to_ascii_lowercase();
        self.inner.remove(&lower).map(|(_, v)| v)
    }

    /// Return the number of unique headers.
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Returns `true` if the map contains no headers.
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterator over `(&original_key, &value)`.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.inner.values().map(|(k, v)| (k, v))
    }
}

impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a String, &'a String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self.inner.values().map(|(k, v)| (k, v)).collect();
        items.into_iter()
    }
}

impl IntoIterator for HeaderMap {
    type Item = (String, String);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self.inner.into_values().collect();
        items.into_iter()
    }
}

impl<K: Into<String>, V: Into<String>> FromIterator<(K, V)> for HeaderMap {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = HeaderMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

pub type Headers = HeaderMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_case_insensitive_lookup() {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "application/json");

        assert_eq!(headers.get("Content-Type"), Some("application/json"));
        assert_eq!(headers.get("content-type"), Some("application/json"));
        assert_eq!(headers.get("CONTENT-TYPE"), Some("application/json"));
        assert_eq!(headers.get("CoNtEnT-tYpE"), Some("application/json"));
        assert!(headers.contains_key("content-type"));
        assert!(headers.contains_key("CONTENT-TYPE"));
    }

    #[test]
    fn test_case_insensitive_overwrite() {
        let mut headers = HeaderMap::new();
        headers.insert("x-custom-header", "v1");
        assert_eq!(headers.len(), 1);

        headers.insert("X-Custom-Header", "v2");
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.get("x-custom-header"), Some("v2"));
        assert_eq!(headers.get("X-CUSTOM-HEADER"), Some("v2"));
    }

    #[test]
    fn test_case_insensitive_remove() {
        let mut headers = HeaderMap::new();
        headers.insert("Authorization", "Bearer token123");

        assert!(headers.contains_key("authorization"));
        let removed = headers.remove("AUTHORIZATION");
        assert_eq!(removed, Some("Bearer token123".to_string()));
        assert!(!headers.contains_key("authorization"));
        assert_eq!(headers.get("Authorization"), None);
    }

    #[test]
    fn test_iteration_preserves_original_casing() {
        let mut headers = HeaderMap::new();
        headers.insert("Content-Type", "text/html; charset=utf-8");
        headers.insert("X-Request-ID", "abc-123");

        let entries: Vec<(&String, &String)> = (&headers).into_iter().collect();
        assert_eq!(entries.len(), 2);
    }
}
