use anyhow::{Context, Result, anyhow};
use image::GenericImageView;
use serde_json::Value;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use uuid::Uuid;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|p| p.parent())
        .unwrap()
        .to_path_buf()
}

/// Attempts to capture a screenshot for the given permalink using the Playwright helper.
/// Returns `None` when Playwright is not available or credentials are missing.
pub fn try_screenshot(permalink: &str) -> Option<PathBuf> {
    let email = std::env::var("TEST_LOGIN_EMAIL").ok();
    let password = std::env::var("TEST_LOGIN_PASSWORD").ok();

    let email = email?;
    let password = password?;

    let tool_dir = repo_root().join("tools/playwright");
    if !tool_dir.join("index.mjs").exists() {
        return None;
    }

    let output_dir = tool_dir.join("output");
    if fs::create_dir_all(&output_dir).is_err() {
        return None;
    }

    let file_name = format!("playwright-{}.png", Uuid::new_v4().simple());
    let output_path = output_dir.join(file_name);

    let status = Command::new("node")
        .current_dir(&tool_dir)
        .arg("index.mjs")
        .arg("--permalink")
        .arg(permalink)
        .arg("--email")
        .arg(&email)
        .arg("--password")
        .arg(&password)
        .arg("--out")
        .arg(output_path.to_string_lossy().to_string())
        .status()
        .ok()?;

    if status.success() && output_path.exists() {
        Some(output_path)
    } else {
        None
    }
}

fn render_with_renderer(card: &Value) -> Result<PathBuf> {
    let tool_dir = repo_root().join("tools/renderers");
    if !tool_dir.join("render.js").exists() {
        return Err(anyhow!(
            "adaptive card renderer missing at {}",
            tool_dir.display()
        ));
    }

    let temp_dir = std::env::temp_dir().join(format!("adaptive-card-{}", Uuid::new_v4().simple()));
    fs::create_dir_all(&temp_dir)
        .with_context(|| format!("failed to create temp dir {}", temp_dir.display()))?;

    let input_path = temp_dir.join("card.json");
    let output_path = temp_dir.join("card.png");

    let payload = serde_json::to_string_pretty(card).context("failed to serialise card payload")?;
    fs::write(&input_path, payload).context("failed to write card payload")?;

    let status = Command::new("node")
        .current_dir(&tool_dir)
        .arg("render.js")
        .arg("--in")
        .arg(input_path.to_string_lossy().to_string())
        .arg("--out")
        .arg(output_path.to_string_lossy().to_string())
        .status()
        .context("failed to spawn adaptive card renderer")?;

    if !status.success() {
        return Err(anyhow!("renderer exited with status {status}"));
    }

    if !output_path.exists() {
        return Err(anyhow!(
            "renderer did not produce output file at {}",
            output_path.display()
        ));
    }

    Ok(output_path)
}

/// Renders an Adaptive Card payload to a PNG using the renderer tool.
pub fn render_adaptive_card_to_png(card: &Value) -> PathBuf {
    render_with_renderer(card).unwrap_or_else(|err| panic!("failed to render adaptive card: {err}"))
}

/// Fallible variant that surfaces renderer errors to the caller.
pub fn try_render_adaptive_card_to_png(card: &Value) -> Result<PathBuf> {
    render_with_renderer(card)
}

fn load_luma(path: &Path) -> Result<(Vec<f64>, u32, u32)> {
    let image =
        image::open(path).with_context(|| format!("failed to load image {}", path.display()))?;
    let (width, height) = image.dimensions();
    let rgba = image.to_rgba8();
    let mut values = Vec::with_capacity((width * height) as usize);
    for pixel in rgba.pixels() {
        let [r, g, b, a] = pixel.0;
        let alpha = a as f64 / 255.0;
        let luminance = (0.2126 * r as f64 + 0.7152 * g as f64 + 0.0722 * b as f64) * alpha
            + (1.0 - alpha) * 255.0;
        values.push(luminance);
    }
    Ok((values, width, height))
}

fn compute_ssim(expected: &Path, actual: &Path) -> Result<f32> {
    let (expected_values, width_a, height_a) = load_luma(expected)?;
    let (actual_values, width_b, height_b) = load_luma(actual)?;

    if width_a != width_b || height_a != height_b {
        return Err(anyhow!(
            "image dimensions mismatch: expected {}x{}, got {}x{}",
            width_a,
            height_a,
            width_b,
            height_b
        ));
    }

    let n = expected_values.len();
    if n == 0 {
        return Err(anyhow!("images contain no pixels"));
    }

    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    let mean_a = mean(&expected_values);
    let mean_b = mean(&actual_values);

    let variance = |values: &[f64], mean: f64| {
        values
            .iter()
            .map(|v| {
                let diff = v - mean;
                diff * diff
            })
            .sum::<f64>()
            / values.len() as f64
    };

    let var_a = variance(&expected_values, mean_a);
    let var_b = variance(&actual_values, mean_b);

    let covariance = expected_values
        .iter()
        .zip(actual_values.iter())
        .map(|(a, b)| (a - mean_a) * (b - mean_b))
        .sum::<f64>()
        / n as f64;

    let c1: f64 = (0.01f64 * 255.0f64).powi(2);
    let c2: f64 = (0.03f64 * 255.0f64).powi(2);

    let numerator = (2.0 * mean_a * mean_b + c1) * (2.0 * covariance + c2);
    let denominator = (mean_a.powi(2) + mean_b.powi(2) + c1) * (var_a + var_b + c2);

    Ok((numerator / denominator) as f32)
}

/// Returns the SSIM similarity between the two images.
pub fn image_similarity(expected: &Path, actual: &Path) -> Result<f32> {
    compute_ssim(expected, actual)
}

#[macro_export]
macro_rules! assert_image_similar {
    ($expected:expr, $actual:expr, ssim_threshold: $threshold:expr $(,)?) => {{
        let expected_path = std::path::Path::new($expected);
        let actual_path = std::path::Path::new($actual);
        let score = $crate::visual::image_similarity(expected_path, actual_path)
            .expect("failed to compare images");
        assert!(
            score >= $threshold,
            "image similarity below threshold: score={:.4}, threshold={:.4}",
            score,
            $threshold
        );
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn render_weather_card_fixture() {
        let sample_path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("cards/samples/weather.json");
        let path_string = sample_path.to_string_lossy().to_string();
        let sample = crate::load_card_value(&path_string).expect("load weather card");
        let output = try_render_adaptive_card_to_png(&sample).expect("render card");
        assert!(output.exists(), "expected output file to exist");
    }
}
