//! Tono de la imagen sobre el área de dibujo: 0 = blanco, 1 = negro.

use image::GrayImage;

/// Cómo se lee la imagen.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Ajuste {
    /// −1 (más claro) … 1 (más oscuro).
    #[serde(rename = "brillo", default)]
    pub brillo: f32,
    /// 0.3 … 3 (1 = sin cambio).
    #[serde(rename = "contraste", default = "uno")]
    pub contraste: f32,
    /// Negativo (lo claro se vuelve oscuro).
    #[serde(rename = "invertir", default)]
    pub invertir: bool,
    /// Estirar los tonos de la imagen para usar del blanco al negro.
    #[serde(rename = "niveles_automaticos", default = "si")]
    pub niveles: bool,
}

fn si() -> bool {
    true
}

fn uno() -> f32 {
    1.0
}

impl Default for Ajuste {
    fn default() -> Ajuste {
        Ajuste { brillo: 0.0, contraste: 1.0, invertir: false, niveles: true }
    }
}

impl Ajuste {
    pub fn aplicar(&self, oscuro: f32) -> f32 {
        let d = if self.invertir { 1.0 - oscuro } else { oscuro };
        ((d - 0.5) * self.contraste + 0.5 + self.brillo * 0.5).clamp(0.0, 1.0)
    }
}

/// Oscuridad de la imagen en una rejilla que cubre el área de dibujo.
#[derive(Clone)]
pub struct Tono {
    pub w: usize,
    pub h: usize,
    /// Oscuridad sin ajustar (0–1).
    pub v: Vec<f32>,
    pub ancho: f32,
    pub alto: f32,
    /// Oscuridad en el 1 % más claro y en el 1 % más oscuro (niveles automáticos).
    pub bajo: f32,
    pub alto_nivel: f32,
}

impl Tono {
    /// La imagen se estira al área (que ya tiene sus proporciones) y se
    /// suaviza a la escala del patrón: el detalle más fino que se puede dibujar
    /// es más o menos medio periodo.
    pub fn new(img: &GrayImage, ancho: f32, alto: f32, periodo: f32) -> Tono {
        let paso = (periodo / 4.0).max(0.25);
        let w = ((ancho / paso).round() as u32).clamp(2, 4000);
        let h = ((alto / paso).round() as u32).clamp(2, 4000);
        let r = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
        // ≈ medio periodo: sin escalones bruscos de grosor donde la imagen cambia de golpe.
        let r = image::imageops::blur(&r, 1.8);
        // Oscuridad perceptual (el valor del archivo): con trazos tan gruesos, la
        // luz lineal oscurecía demasiado los tonos medios y la imagen no se leía.
        let v = r.pixels().map(|p| 1.0 - p.0[0] as f32 / 255.0).collect();
        let v: Vec<f32> = v;
        let mut orden = v.clone();
        orden.sort_by(f32::total_cmp);
        let q = |f: f32| orden[((orden.len() - 1) as f32 * f) as usize];
        let (bajo, alto_nivel) = (q(0.01), q(0.99));
        Tono { w: w as usize, h: h as usize, v, ancho, alto, bajo, alto_nivel }
    }

    /// Oscuridad con los niveles estirados (si la imagen usa poco rango).
    pub fn nivelada(&self, x: f32, y: f32) -> f32 {
        let r = self.alto_nivel - self.bajo;
        let t = self.muestra(x, y);
        if r < 0.05 { t } else { ((t - self.bajo) / r).clamp(0.0, 1.0) }
    }

    /// Oscuridad en (x, y) mm (bilineal; fuera del área, la del borde).
    pub fn muestra(&self, x: f32, y: f32) -> f32 {
        let fx = (x / self.ancho * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let fy = (y / self.alto * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (i1, j1) = ((i + 1).min(self.w - 1), (j + 1).min(self.h - 1));
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let at = |i: usize, j: usize| self.v[j * self.w + i];
        let a = at(i, j) * (1.0 - tx) + at(i1, j) * tx;
        let b = at(i, j1) * (1.0 - tx) + at(i1, j1) * tx;
        a * (1.0 - ty) + b * ty
    }
}

/// La tinta cubre superficie: para que de lejos el gris se vea igual, se
/// trabaja con la luz real (lineal), no con el valor del archivo (sRGB).
#[allow(dead_code)]
fn srgb_a_lineal(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// Abre una imagen (PNG, JPEG, GIF, WebP, BMP) en escala de grises. Lo
/// transparente cuenta como blanco.
#[cfg(test)]
pub fn abrir(path: &std::path::Path) -> Result<GrayImage, String> {
    Ok(gris(&image::open(path).map_err(|e| e.to_string())?))
}

/// Imagen → gris (luminancia; lo transparente cuenta como blanco).
pub fn gris(img: &image::DynamicImage) -> GrayImage {
    let rgba = img.to_rgba8();
    let mut g = GrayImage::new(rgba.width(), rgba.height());
    for (p, q) in rgba.pixels().zip(g.pixels_mut()) {
        let a = p.0[3] as f32 / 255.0;
        let l = 0.2126 * p.0[0] as f32 + 0.7152 * p.0[1] as f32 + 0.0722 * p.0[2] as f32;
        q.0[0] = (l * a + 255.0 * (1.0 - a)).round() as u8;
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradado_de_blanco_a_negro() {
        let img = GrayImage::from_fn(100, 10, |x, _| image::Luma([255 - (x * 255 / 99) as u8]));
        let t = Tono::new(&img, 100.0, 10.0, 4.0);
        assert!(t.muestra(2.0, 5.0) < 0.1);
        assert!(t.muestra(98.0, 5.0) > 0.9);
        assert!((t.muestra(50.0, 5.0) - 0.5).abs() < 0.1, "gris medio ≈ 0.5 de oscuridad");
    }

    #[test]
    fn ajustes() {
        let a = Ajuste { invertir: true, ..Default::default() };
        assert_eq!(a.aplicar(0.2), 0.8);
        let c = Ajuste { contraste: 2.0, ..Default::default() };
        assert_eq!(c.aplicar(0.75), 1.0);
        assert_eq!(c.aplicar(0.5), 0.5);
    }
}
