use std::borrow::Cow;

/// A lightweight, low-allocation case-insensitive HTTP header map.
///
/// Header field names are case-insensitive according to RFC 9110 (Section 5.1).
/// Uses an inline/small vector structure where keys and values can be borrowed (`Cow::Borrowed`)
/// or owned (`Cow::Owned`), avoiding unnecessary heap allocations on parse and lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    // Stores (original_key, lowercased_key, value)
    entries: Vec<(Cow<'static, str>, String, Cow<'static, str>)>,
}

impl HeaderMap {
    /// Create a new, empty `HeaderMap`.
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Create a new `HeaderMap` with pre-allocated capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            entries: Vec::with_capacity(capacity),
        }
    }

    /// Insert a static header key and value with 0 heap allocations for string buffers.
    pub fn insert_static(&mut self, key: &'static str, value: &'static str) {
        let lower = key.to_ascii_lowercase();
        if let Some(entry) = self.entries.iter_mut().find(|(_, l, _)| l == &lower) {
            entry.0 = Cow::Borrowed(key);
            entry.2 = Cow::Borrowed(value);
        } else {
            self.entries.push((Cow::Borrowed(key), lower, Cow::Borrowed(value)));
        }
    }

    /// Insert a header name-value pair.
    ///
    /// If a header with the same name (case-insensitive) already exists,
    /// its value and casing are updated.
    pub fn insert(&mut self, key: impl Into<String>, value: impl Into<String>) {
        let key_str = key.into();
        let val_str = value.into();
        let lower = key_str.to_ascii_lowercase();

        if let Some(entry) = self.entries.iter_mut().find(|(_, l, _)| l == &lower) {
            entry.0 = Cow::Owned(key_str);
            entry.2 = Cow::Owned(val_str);
        } else {
            self.entries.push((Cow::Owned(key_str), lower, Cow::Owned(val_str)));
        }
    }

    pub fn insert_cow(&mut self, key: Cow<'static, str>, value: Cow<'static, str>) {
        let lower = key.to_ascii_lowercase();
        if let Some(entry) = self.entries.iter_mut().find(|(_, l, _)| l == &lower) {
            entry.0 = key;
            entry.2 = value;
        } else {
            self.entries.push((key, lower, value));
        }
    }

    /// Get a header value by name (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(_, l, _)| l.eq_ignore_ascii_case(key))
            .map(|(_, _, v)| v.as_ref())
    }

    /// Check if a header exists (case-insensitive).
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|(_, l, _)| l.eq_ignore_ascii_case(key))
    }

    /// Remove a header by name (case-insensitive) and return its value if present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|(_, l, _)| l.eq_ignore_ascii_case(key))
        {
            Some(self.entries.remove(idx).2.into_owned())
        } else {
            None
        }
    }

    /// Return the number of unique headers.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the map contains no headers.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterator over `(&original_key, &value)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(k, _, v)| (k.as_ref(), v.as_ref()))
    }
}

impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a str, &'a str);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self
            .entries
            .iter()
            .map(|(k, _, v)| (k.as_ref(), v.as_ref()))
            .collect();
        items.into_iter()
    }
}

impl IntoIterator for HeaderMap {
    type Item = (Cow<'static, str>, Cow<'static, str>);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self
            .entries
            .into_iter()
            .map(|(k, _, v)| (k, v))
            .collect();
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
