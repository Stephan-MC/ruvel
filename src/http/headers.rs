use std::borrow::Cow;

/// A lightweight, low-allocation case-insensitive HTTP header map.
///
/// Header field names are case-insensitive according to RFC 9110 (Section 5.1).
/// Uses an inline/small vector structure where keys and values can be borrowed (`Cow::Borrowed`)
/// or owned (`Cow::Owned`), avoiding unnecessary heap allocations on parse and lookup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderMap {
    // Stores (original_key, value)
    entries: Vec<(Cow<'static, str>, Cow<'static, str>)>,
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

    /// Create a `HeaderMap` with a single initial entry, avoiding over-allocation and
    /// duplicate-key scanning when constructing standard responses.
    pub(crate) fn with_single(
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) -> Self {
        Self {
            entries: vec![(key.into(), value.into())],
        }
    }

    /// Insert a header name-value pair.
    ///
    /// Accepts `&'static str` (0 heap allocations), `String`, or `Cow<'static, str>`.
    /// If a header with the same name (case-insensitive) already exists,
    /// its value and casing are updated.
    pub fn insert(
        &mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<Cow<'static, str>>,
    ) {
        let key = key.into();
        let value = value.into();

        if let Some(entry) = self.entries.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(&key)) {
            entry.0 = key;
            entry.1 = value;
        } else {
            self.entries.push((key, value));
        }
    }

    /// Get a header value by name (case-insensitive).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(key))
            .map(|(_, v)| v.as_ref())
    }

    /// Check if a header exists (case-insensitive).
    pub fn contains_key(&self, key: &str) -> bool {
        self.entries
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case(key))
    }

    /// Remove a header by name (case-insensitive) and return its value if present.
    pub fn remove(&mut self, key: &str) -> Option<String> {
        if let Some(idx) = self
            .entries
            .iter()
            .position(|(k, _)| k.eq_ignore_ascii_case(key))
        {
            Some(self.entries.remove(idx).1.into_owned())
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
            .map(|(k, v)| (k.as_ref(), v.as_ref()))
    }
}

impl<'a> IntoIterator for &'a HeaderMap {
    type Item = (&'a str, &'a str);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        let items: Vec<Self::Item> = self
            .entries
            .iter()
            .map(|(k, v)| (k.as_ref(), v.as_ref()))
            .collect();
        items.into_iter()
    }
}

impl IntoIterator for HeaderMap {
    type Item = (Cow<'static, str>, Cow<'static, str>);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.into_iter()
    }
}

impl<K: Into<Cow<'static, str>>, V: Into<Cow<'static, str>>> FromIterator<(K, V)> for HeaderMap {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut map = HeaderMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

pub type Headers = HeaderMap;
