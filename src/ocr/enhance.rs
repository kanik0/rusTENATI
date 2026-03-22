use std::path::Path;

use anyhow::{Context, Result};
use image::{DynamicImage, GrayImage, ImageBuffer, Luma};
use tracing::debug;

/// Image enhancement pipeline for pre-OCR processing of historical documents.
/// Improves OCR accuracy on 18th-19th century Italian civil records by:
/// - Converting to grayscale
/// - Applying contrast enhancement (adaptive threshold)
/// - Denoising (median filter)
/// - Binarization (Otsu's threshold)
pub struct ImageEnhancer {
    pub contrast: bool,
    pub denoise: bool,
    pub binarize: bool,
}

impl Default for ImageEnhancer {
    fn default() -> Self {
        Self {
            contrast: true,
            denoise: true,
            binarize: false, // disabled by default as it can harm some documents
        }
    }
}

impl ImageEnhancer {
    /// Enhance an image file and save to the output path. Returns the enhanced image path.
    pub fn enhance(&self, input: &Path, output: &Path) -> Result<()> {
        let img = image::open(input)
            .with_context(|| format!("Failed to open image: {}", input.display()))?;

        debug!("Enhancing image: {} ({}x{})", input.display(), img.width(), img.height());

        let mut gray = img.to_luma8();

        if self.contrast {
            gray = enhance_contrast(&gray);
            debug!("Applied contrast enhancement");
        }

        if self.denoise {
            gray = median_filter(&gray, 1);
            debug!("Applied denoising (median filter)");
        }

        if self.binarize {
            gray = otsu_threshold(&gray);
            debug!("Applied Otsu binarization");
        }

        let enhanced = DynamicImage::ImageLuma8(gray);
        enhanced.save(output)
            .with_context(|| format!("Failed to save enhanced image: {}", output.display()))?;

        debug!("Saved enhanced image: {}", output.display());
        Ok(())
    }

    /// Enhance an image in-memory and return the enhanced image.
    pub fn enhance_image(&self, img: &DynamicImage) -> DynamicImage {
        let mut gray = img.to_luma8();

        if self.contrast {
            gray = enhance_contrast(&gray);
        }

        if self.denoise {
            gray = median_filter(&gray, 1);
        }

        if self.binarize {
            gray = otsu_threshold(&gray);
        }

        DynamicImage::ImageLuma8(gray)
    }
}

/// Enhance contrast using histogram stretching (min-max normalization).
/// Maps the darkest pixels to 0 and brightest to 255, stretching the range.
fn enhance_contrast(img: &GrayImage) -> GrayImage {
    let (width, height) = img.dimensions();

    // Find min and max pixel values
    let mut min_val = 255u8;
    let mut max_val = 0u8;
    for pixel in img.pixels() {
        let v = pixel[0];
        if v < min_val { min_val = v; }
        if v > max_val { max_val = v; }
    }

    if max_val == min_val {
        return img.clone();
    }

    let range = (max_val - min_val) as f32;
    let mut out = ImageBuffer::new(width, height);

    for (x, y, pixel) in img.enumerate_pixels() {
        let v = ((pixel[0] - min_val) as f32 / range * 255.0) as u8;
        out.put_pixel(x, y, Luma([v]));
    }

    out
}

/// Simple 3x3 median filter for noise reduction.
/// `radius` controls the filter size: 1 = 3x3, 2 = 5x5.
fn median_filter(img: &GrayImage, radius: u32) -> GrayImage {
    let (width, height) = img.dimensions();
    let mut out = ImageBuffer::new(width, height);
    let size = (2 * radius + 1) as usize;
    let mut window = Vec::with_capacity(size * size);

    for y in 0..height {
        for x in 0..width {
            window.clear();
            for dy in 0..size {
                for dx in 0..size {
                    let nx = (x as i64 + dx as i64 - radius as i64).clamp(0, width as i64 - 1) as u32;
                    let ny = (y as i64 + dy as i64 - radius as i64).clamp(0, height as i64 - 1) as u32;
                    window.push(img.get_pixel(nx, ny)[0]);
                }
            }
            window.sort_unstable();
            let median = window[window.len() / 2];
            out.put_pixel(x, y, Luma([median]));
        }
    }

    out
}

/// Otsu's method for automatic binarization threshold.
/// Finds the threshold that minimizes intra-class variance.
fn otsu_threshold(img: &GrayImage) -> GrayImage {
    let (width, height) = img.dimensions();
    let total_pixels = (width * height) as f64;

    // Build histogram
    let mut histogram = [0u32; 256];
    for pixel in img.pixels() {
        histogram[pixel[0] as usize] += 1;
    }

    // Compute Otsu's threshold
    let mut sum = 0.0f64;
    for (i, &count) in histogram.iter().enumerate() {
        sum += i as f64 * count as f64;
    }

    let mut sum_b = 0.0f64;
    let mut w_b = 0.0f64;
    let mut max_variance = 0.0f64;
    let mut threshold = 0u8;

    for (t, &count) in histogram.iter().enumerate() {
        w_b += count as f64;
        if w_b == 0.0 { continue; }

        let w_f = total_pixels - w_b;
        if w_f == 0.0 { break; }

        sum_b += t as f64 * count as f64;

        let mean_b = sum_b / w_b;
        let mean_f = (sum - sum_b) / w_f;

        let variance = w_b * w_f * (mean_b - mean_f) * (mean_b - mean_f);

        if variance > max_variance {
            max_variance = variance;
            threshold = t as u8;
        }
    }

    // Apply threshold
    let mut out = ImageBuffer::new(width, height);
    for (x, y, pixel) in img.enumerate_pixels() {
        let v = if pixel[0] > threshold { 255 } else { 0 };
        out.put_pixel(x, y, Luma([v]));
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_otsu_threshold_uniform() {
        // Uniform image should not crash
        let img = ImageBuffer::from_fn(10, 10, |_, _| Luma([128u8]));
        let result = otsu_threshold(&img);
        assert_eq!(result.dimensions(), (10, 10));
    }

    #[test]
    fn test_contrast_enhancement() {
        let mut img = ImageBuffer::new(4, 4);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            *pixel = Luma([((x + y * 4) * 10 + 50) as u8]);
        }
        let result = enhance_contrast(&img);
        // Min should be 0, max should be 255
        let min = result.pixels().map(|p| p[0]).min().unwrap();
        let max = result.pixels().map(|p| p[0]).max().unwrap();
        assert_eq!(min, 0);
        assert_eq!(max, 255);
    }

    #[test]
    fn test_median_filter_identity() {
        let img = ImageBuffer::from_fn(5, 5, |_, _| Luma([100u8]));
        let result = median_filter(&img, 1);
        // Uniform image should remain unchanged
        for pixel in result.pixels() {
            assert_eq!(pixel[0], 100);
        }
    }
}
