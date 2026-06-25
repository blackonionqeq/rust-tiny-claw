use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct FileMemory {
    root: PathBuf,
}

impl FileMemory {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}
