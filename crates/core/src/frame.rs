// The two shapes every stage passes around: a frame size, and RGB8 pixels that know theirs.

use anyhow::{Result, ensure};

/// One type for every width-height pair, so the two cannot be swapped or cast differently at each use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Size {
    pub w: u32,
    pub h: u32,
}

impl Size {
    pub const fn new(w: u32, h: u32) -> Self {
        Self { w, h }
    }

    pub fn px(self) -> usize {
        self.w as usize * self.h as usize
    }

    pub fn is_empty(self) -> bool {
        self.w == 0 || self.h == 0
    }

    /// For gyroflow, which counts in `usize`.
    pub fn as_usize(self) -> (usize, usize) {
        (self.w as usize, self.h as usize)
    }

    /// The same aspect at width `w`, at least one pixel tall.
    pub fn at_width(self, w: u32) -> Self {
        let h = (self.h as u64 * w as u64 / self.w.max(1) as u64).max(1) as u32;
        Self { w, h }
    }
}

impl std::fmt::Display for Size {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}x{}", self.w, self.h)
    }
}

/// Tightly packed RGB8 pixels, row-major, stride `w * 3`. The length always matches the size.
#[derive(Clone, PartialEq, Eq)]
pub struct Rgb8 {
    size: Size,
    data: Vec<u8>,
}

impl Rgb8 {
    pub fn new(size: Size, data: Vec<u8>) -> Result<Self> {
        ensure!(
            data.len() == size.px() * 3,
            "{size} RGB frame needs {} bytes, got {}",
            size.px() * 3,
            data.len()
        );
        Ok(Self { size, data })
    }

    pub fn zeroed(size: Size) -> Self {
        Self {
            size,
            data: vec![0; size.px() * 3],
        }
    }

    pub fn size(&self) -> Size {
        self.size
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// A slice, so the length cannot change under the size.
    pub fn data_mut(&mut self) -> &mut [u8] {
        &mut self.data
    }

    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
}
