use std::collections::HashMap;
use std::sync::Mutex;

use crate::engine::summary_tree::recall::RecallLine;

const FIND_CACHE_ENTRIES: usize = 20_000;

#[derive(Default)]
pub struct FindCache(Mutex<HashMap<(String, String, String), f64>>);

impl FindCache {
    pub fn get(&self, query: &str, line: &RecallLine) -> Option<f64> {
        let key = (query.to_string(), line.id.clone(), line.text.clone());
        self.0.lock().unwrap().get(&key).copied()
    }

    pub fn put(&self, query: &str, line: &RecallLine, p: f64) {
        let mut cache = self.0.lock().unwrap();
        if cache.len() >= FIND_CACHE_ENTRIES {
            cache.clear();
        }
        cache.insert((query.to_string(), line.id.clone(), line.text.clone()), p);
    }
}
