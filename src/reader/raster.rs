use ndarray::Array3;
use std::sync::Arc;

pub trait RasterSource: Send + Sync {
    fn bands(&self) -> usize;
    fn width(&self) -> usize;
    fn height(&self) -> usize;
    fn sample(&self, band: usize, x: usize, y: usize) -> Option<f32>;
}

pub struct ArrayRasterSource {
    data: Arc<Array3<f32>>,
}

impl ArrayRasterSource {
    #[must_use] pub fn new(data: Arc<Array3<f32>>) -> Self {
        Self { data }
    }

    #[must_use] pub fn inner(&self) -> &Arc<Array3<f32>> {
        &self.data
    }
}

impl RasterSource for ArrayRasterSource {
    fn bands(&self) -> usize {
        self.data.dim().0
    }

    fn width(&self) -> usize {
        self.data.dim().2
    }

    fn height(&self) -> usize {
        self.data.dim().1
    }

    fn sample(&self, band: usize, x: usize, y: usize) -> Option<f32> {
        let (bands, height, width) = self.data.dim();
        if band >= bands || x >= width || y >= height {
            return None;
        }
        Some(self.data[[band, y, x]])
    }
}
