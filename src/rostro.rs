//! Rasgos de un rostro: detector de caras y malla de 468 puntos de MediaPipe
//! (Apache-2.0, `modelos/`), ejecutados con tract en Rust puro.
//!
//! 1. Detector (BlazeFace de rango completo, 192×192): caja y 6 puntos (ojos,
//!    nariz, boca, orejas) de la cara con más puntuación.
//! 2. Malla (192×192): el recorte cuadrado de la cara (1,5 × la caja, girado
//!    para que los ojos queden horizontales) → 468 puntos. Se repite una vez con
//!    el recorte calculado de la propia malla (más preciso).
//!
//! Los puntos se guardan normalizados (0–1 del ancho y del alto de la imagen):
//! la imagen se estira al área de dibujo, así que en mm son (u·ancho, v·alto).

use image::RgbImage;
use std::sync::OnceLock;
use tract_tflite::prelude::*;

type Modelo = std::sync::Arc<TypedRunnableModel>;

static DETECTOR: &[u8] = include_bytes!("../modelos/face_detection_full_range.tflite");
static MALLA: &[u8] = include_bytes!("../modelos/face_landmark.tflite");

fn cargar(datos: &[u8]) -> Option<Modelo> {
    let m = tract_tflite::tflite().model_for_read(&mut std::io::Cursor::new(datos)).ok()?;
    m.into_optimized().ok()?.into_runnable().ok()
}

fn modelos() -> Option<&'static (Modelo, Modelo)> {
    static M: OnceLock<Option<(Modelo, Modelo)>> = OnceLock::new();
    M.get_or_init(|| Some((cargar(DETECTOR)?, cargar(MALLA)?))).as_ref()
}

/// Rasgos de un rostro.
#[derive(Clone, Debug, PartialEq)]
pub struct Rostro {
    /// Los 468 puntos de la malla, normalizados a la imagen (u, v).
    pub puntos: Vec<(f32, f32)>,
}

/// Contornos de la malla de MediaPipe (índices de puntos, en orden).
pub mod indices {
    /// Óvalo de la cara (cerrado), desde la frente en el sentido de las agujas del reloj (en la imagen).
    pub const OVALO: &[usize] = &[
        10, 338, 297, 332, 284, 251, 389, 356, 454, 323, 361, 288, 397, 365, 379, 378, 400, 377, 152, 148, 176, 149, 150, 136, 172, 58, 132, 93, 234,
        127, 162, 21, 54, 103, 67, 109,
    ];
    /// Ojo a la izquierda de la imagen (el derecho de la persona), cerrado.
    pub const OJO_I: &[usize] = &[33, 7, 163, 144, 145, 153, 154, 155, 133, 173, 157, 158, 159, 160, 161, 246];
    /// Ojo a la derecha de la imagen, cerrado.
    pub const OJO_D: &[usize] = &[263, 249, 390, 373, 374, 380, 381, 382, 362, 398, 384, 385, 386, 387, 388, 466];
    /// Cejas (borde de arriba, de fuera hacia dentro… y el de abajo).
    pub const CEJA_I: &[usize] = &[70, 63, 105, 66, 107, 55, 65, 52, 53, 46];
    pub const CEJA_D: &[usize] = &[300, 293, 334, 296, 336, 285, 295, 282, 283, 276];
    /// Labios: contorno exterior (cerrado).
    pub const LABIOS: &[usize] = &[61, 146, 91, 181, 84, 17, 314, 405, 321, 375, 291, 409, 270, 269, 267, 0, 37, 39, 40, 185];
    /// Línea entre los labios.
    pub const BOCA: &[usize] = &[78, 95, 88, 178, 87, 14, 317, 402, 318, 324, 308];
    /// Puente de la nariz, del entrecejo a la punta.
    pub const PUENTE: &[usize] = &[168, 6, 197, 195, 5, 4, 1];
    /// Base de la nariz (aletas y columela).
    pub const BASE_NARIZ: &[usize] = &[98, 97, 2, 326, 327];
}

impl Rostro {
    /// Dónde se dibuja un asa (normalizado a la imagen).
    pub fn asa(&self, a: Asa) -> (f32, f32) {
        match a {
            Asa::Grupo(g) => {
                let idx: Vec<usize> = g.iter().flat_map(|l| l.iter().copied()).collect();
                self.centro(&idx)
            }
            Asa::Ovalo(k) => self.puntos[indices::OVALO[k]],
        }
    }

    /// Mueve un asa (du, dv normalizados).
    pub fn mover(&mut self, a: Asa, du: f32, dv: f32) {
        match a {
            Asa::Grupo(g) => {
                let mut idx: Vec<usize> = g.iter().flat_map(|l| l.iter().copied()).collect();
                idx.sort_unstable();
                idx.dedup();
                for i in idx {
                    self.puntos[i].0 += du;
                    self.puntos[i].1 += dv;
                }
            }
            Asa::Ovalo(k) => {
                let n = indices::OVALO.len();
                for (j, &i) in indices::OVALO.iter().enumerate() {
                    let d = (j as isize - k as isize).unsigned_abs();
                    let d = d.min(n - d) as f32;
                    if d < 5.0 {
                        let w = (std::f32::consts::FRAC_PI_2 * d / 5.0).cos().powi(2);
                        self.puntos[i].0 += du * w;
                        self.puntos[i].1 += dv * w;
                    }
                }
            }
        }
    }

    #[cfg(test)]
    pub fn punto(&self, i: usize) -> (f32, f32) {
        self.puntos[i]
    }

    /// Centro de un grupo de puntos.
    pub fn centro(&self, idx: &[usize]) -> (f32, f32) {
        let n = idx.len() as f32;
        let (sx, sy) = idx.iter().fold((0.0, 0.0), |(a, b), &i| (a + self.puntos[i].0, b + self.puntos[i].1));
        (sx / n, sy / n)
    }
}

/// Punto que se puede arrastrar para corregir la detección.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Asa {
    /// Mueve juntos todos los puntos de estos contornos.
    Grupo(&'static [&'static [usize]]),
    /// Un punto del óvalo (posición en `OVALO`); arrastra con suavidad a sus vecinos.
    Ovalo(usize),
}

impl Asa {
    pub fn todas() -> Vec<Asa> {
        use indices::*;
        let mut v = vec![
            Asa::Grupo(&[OJO_I]),
            Asa::Grupo(&[OJO_D]),
            Asa::Grupo(&[CEJA_I]),
            Asa::Grupo(&[CEJA_D]),
            Asa::Grupo(&[PUENTE, BASE_NARIZ]),
            Asa::Grupo(&[LABIOS, BOCA]),
        ];
        v.extend([0, 5, 9, 14, 18, 23, 27, 32].map(Asa::Ovalo));
        v
    }
}

/// Contornos que se dibujan sobre la imagen para ver lo detectado (y si es cerrado).
pub const CONTORNOS: &[(&[usize], bool)] = &[
    (indices::OVALO, true),
    (indices::OJO_I, true),
    (indices::OJO_D, true),
    (indices::CEJA_I, false),
    (indices::CEJA_D, false),
    (indices::PUENTE, false),
    (indices::BASE_NARIZ, false),
    (indices::LABIOS, true),
    (indices::BOCA, false),
];

/// Ojo en mm: centro, semiejes (a lo largo y a lo alto) y giro.
#[derive(Clone, Copy, Debug)]
pub struct Ojo {
    pub c: (f32, f32),
    pub a: f32,
    pub b: f32,
    pub ang: f32,
}

impl Ojo {
    /// Distancia «elíptica» (1 = borde del ojo) y dirección hacia fuera (normal de la elipse).
    pub fn polar(&self, x: f32, y: f32) -> (f32, (f32, f32)) {
        let (s, c) = self.ang.sin_cos();
        let (dx, dy) = (x - self.c.0, y - self.c.1);
        let (u, v) = (dx * c + dy * s, -dx * s + dy * c);
        let rho = ((u / self.a).powi(2) + (v / self.b).powi(2)).sqrt();
        // Gradiente de la elipse, de vuelta a la imagen.
        let (gu, gv) = (u / (self.a * self.a), v / (self.b * self.b));
        let (gx, gy) = (gu * c - gv * s, gu * s + gv * c);
        let l = (gx * gx + gy * gy).sqrt().max(1e-9);
        (rho, (gx / l, gy / l))
    }
}

/// El rostro en mm sobre el área de dibujo: lo que usan las líneas de flujo.
#[derive(Clone, Debug)]
pub struct Cara {
    /// Óvalo (cerrado).
    pub ovalo: Vec<(f32, f32)>,
    pub ojos: [Ojo; 2],
    /// Curvas que guían las líneas (cejas, nariz, labios): las líneas corren paralelas a ellas.
    pub guias: Vec<Vec<(f32, f32)>>,
    /// Curvas de los rasgos (ojos, cejas, nariz, labios): cerca de ellas, más detalle.
    pub rasgos: Vec<Vec<(f32, f32)>>,
    /// Centro y semiejes del óvalo.
    pub centro: (f32, f32),
    pub semi: (f32, f32),
}

impl Cara {
    pub fn new(r: &Rostro, ancho: f32, alto: f32) -> Cara {
        let mm = |i: usize| (r.puntos[i].0 * ancho, r.puntos[i].1 * alto);
        let curva = |idx: &[usize]| idx.iter().map(|&i| mm(i)).collect::<Vec<_>>();
        let cerrada = |idx: &[usize]| {
            let mut v = curva(idx);
            v.push(v[0]);
            v
        };
        let ojo = |idx: &[usize], esq: (usize, usize), arriba_abajo: (usize, usize)| {
            let pts = curva(idx);
            let n = pts.len() as f32;
            let c = (pts.iter().map(|p| p.0).sum::<f32>() / n, pts.iter().map(|p| p.1).sum::<f32>() / n);
            let (p, q) = (mm(esq.0), mm(esq.1));
            let a = 0.5 * ((q.0 - p.0).hypot(q.1 - p.1));
            let (t, b) = (mm(arriba_abajo.0), mm(arriba_abajo.1));
            let _ = (t, b);
            // Anillos alrededor del iris (círculos de un poco más que la mitad del ojo), no de todo el ojo.
            let r = a * std::env::var("DEDALO_IRIS").ok().and_then(|v| v.parse().ok()).unwrap_or(0.55f32);
            Ojo { c, a: r, b: r, ang: (q.1 - p.1).atan2(q.0 - p.0) }
        };
        use indices::*;
        let ovalo = cerrada(OVALO);
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, y) in &ovalo {
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        let ceja = |idx: &[usize]| curva(&idx[..5]);
        let guias = vec![ceja(CEJA_I), ceja(CEJA_D), curva(PUENTE), curva(BASE_NARIZ), cerrada(LABIOS), curva(BOCA)];
        let mut rasgos = guias.clone();
        rasgos.push(cerrada(OJO_I));
        rasgos.push(cerrada(OJO_D));
        Cara {
            ovalo,
            ojos: [ojo(OJO_I, (33, 133), (159, 145)), ojo(OJO_D, (362, 263), (386, 374))],
            guias,
            rasgos,
            centro: ((x0 + x1) / 2.0, (y0 + y1) / 2.0),
            semi: ((x1 - x0) / 2.0, (y1 - y0) / 2.0),
        }
    }

    /// ¿Dentro del óvalo?
    pub fn dentro(&self, x: f32, y: f32) -> bool {
        let mut d = false;
        let n = self.ovalo.len();
        for k in 0..n - 1 {
            let (a, b) = (self.ovalo[k], self.ovalo[k + 1]);
            if (a.1 > y) != (b.1 > y) && x < a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0) {
                d = !d;
            }
        }
        d
    }

    /// ¿En la zona donde puede estar el pelo? (alrededor de la cabeza, fuera del óvalo).
    pub fn zona_pelo(&self, x: f32, y: f32) -> bool {
        let (cx, cy) = self.centro;
        let (ax, ay) = self.semi;
        let (u, v) = ((x - cx) / (1.55 * ax), (y - (cy - 0.2 * ay)) / (1.5 * ay));
        u * u + v * v < 1.0 && y < cy + 0.5 * ay && !self.dentro(x, y)
    }
}

/// Distancia de (x, y) a una polilínea y su tangente unitaria ahí.
pub fn cerca(l: &[(f32, f32)], x: f32, y: f32) -> (f32, (f32, f32)) {
    let mut mejor = (f32::MAX, (1.0, 0.0));
    for w in l.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let l2 = dx * dx + dy * dy;
        if l2 <= 0.0 {
            continue;
        }
        let t = (((x - a.0) * dx + (y - a.1) * dy) / l2).clamp(0.0, 1.0);
        let d = (a.0 + t * dx - x).hypot(a.1 + t * dy - y);
        if d < mejor.0 {
            let l = l2.sqrt();
            mejor = (d, (dx / l, dy / l));
        }
    }
    mejor
}

/// Imagen → muestra bilineal en píxeles (fuera = gris medio).
fn muestra(img: &RgbImage, x: f32, y: f32) -> [f32; 3] {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let (x0, y0) = ((x - 0.5).floor(), (y - 0.5).floor());
    let (tx, ty) = (x - 0.5 - x0, y - 0.5 - y0);
    let at = |i: i64, j: i64| -> [f32; 3] {
        if i < 0 || j < 0 || i >= w || j >= h {
            return [0.0; 3];
        }
        let p = img.get_pixel(i as u32, j as u32).0;
        [p[0] as f32, p[1] as f32, p[2] as f32]
    };
    let (i, j) = (x0 as i64, y0 as i64);
    let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i, j + 1), at(i + 1, j + 1));
    let mut o = [0.0; 3];
    for k in 0..3 {
        o[k] = (a[k] * (1.0 - tx) + b[k] * tx) * (1.0 - ty) + (c[k] * (1.0 - tx) + d[k] * tx) * ty;
    }
    o
}

/// Recorte cuadrado de `lado` px centrado en (cx, cy) y girado `ang` rad →
/// tensor 1×n×n×3 con `escala(v)` en cada canal.
fn recorte(img: &RgbImage, cx: f32, cy: f32, lado: f32, ang: f32, n: usize, escala: impl Fn(f32) -> f32) -> Tensor {
    let (s, c) = ang.sin_cos();
    let mut v = Vec::with_capacity(n * n * 3);
    for j in 0..n {
        for i in 0..n {
            let (u, w) = (((i as f32 + 0.5) / n as f32 - 0.5) * lado, ((j as f32 + 0.5) / n as f32 - 0.5) * lado);
            let (x, y) = (cx + u * c - w * s, cy + u * s + w * c);
            for k in muestra(img, x, y) {
                v.push(escala(k));
            }
        }
    }
    tract_ndarray::Array4::from_shape_vec((1, n, n, 3), v).unwrap().into()
}

fn sigmoide(x: f32) -> f32 {
    1.0 / (1.0 + (-x.clamp(-100.0, 100.0)).exp())
}

/// Busca el rostro principal. `None` si no hay ninguno claro.
pub fn detectar(img: &RgbImage) -> Option<Rostro> {
    let (det, malla) = modelos()?;
    let (w, h) = (img.width() as f32, img.height() as f32);
    // 1) Detector sobre la imagen entera, encajada en un cuadrado (centrada).
    let lado = w.max(h);
    let entrada = recorte(img, w / 2.0, h / 2.0, lado, 0.0, 192, |v| v / 127.5 - 1.0);
    let r = det.run(tvec!(entrada.into())).ok()?;
    let reg = r[0].to_plain_array_view::<f32>().ok()?;
    let cls = r[1].to_plain_array_view::<f32>().ok()?;
    let mut mejor = (0.0f32, 0usize);
    for a in 0..2304 {
        let p = sigmoide(cls[[0, a, 0]]);
        if p > mejor.0 {
            mejor = (p, a);
        }
    }
    if mejor.0 < 0.5 {
        return None;
    }
    let a = mejor.1;
    let (acx, acy) = (((a % 48) as f32 + 0.5) / 48.0, ((a / 48) as f32 + 0.5) / 48.0);
    // De coordenadas del cuadrado (0–1) a píxeles de la imagen.
    let px = |u: f32, v: f32| ((u - 0.5) * lado + w / 2.0, (v - 0.5) * lado + h / 2.0);
    let g = |k: usize| reg[[0, a, k]] / 192.0;
    let (cx, cy) = px(acx + g(0), acy + g(1));
    let (bw, bh) = (g(2) * lado, g(3) * lado);
    let ojo_i = px(acx + g(4), acy + g(5));
    let ojo_d = px(acx + g(6), acy + g(7));
    let ang = (ojo_d.1 - ojo_i.1).atan2(ojo_d.0 - ojo_i.0);
    // 2) Malla, dos veces.
    let mut roi = (cx, cy, 1.5 * bw.max(bh), ang);
    let mut puntos = vec![];
    for _ in 0..2 {
        let (cx, cy, lado, ang) = roi;
        let entrada = recorte(img, cx, cy, lado, ang, 192, |v| v / 255.0);
        let r = malla.run(tvec!(entrada.into())).ok()?;
        let p = r[0].to_plain_array_view::<f32>().ok()?;
        let cara = sigmoide(r[1].to_plain_array_view::<f32>().ok()?.iter().next().copied()?);
        if cara < 0.5 {
            return None;
        }
        let (s, c) = ang.sin_cos();
        puntos = (0..468)
            .map(|i| {
                let (u, v) = ((p[[0, 0, 0, i * 3]] / 192.0 - 0.5) * lado, (p[[0, 0, 0, i * 3 + 1]] / 192.0 - 0.5) * lado);
                (cx + u * c - v * s, cy + u * s + v * c)
            })
            .collect();
        // Recorte siguiente: caja de los puntos (en el marco girado por los ojos).
        let (a, b) = (puntos[33], puntos[263]);
        let ang = (b.1 - a.1).atan2(b.0 - a.0);
        let (s, c) = ang.sin_cos();
        let rot: Vec<(f32, f32)> = puntos.iter().map(|&(x, y)| (x * c + y * s, -x * s + y * c)).collect();
        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for &(x, y) in &rot {
            (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
        }
        let (mx, my) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);
        roi = (mx * c - my * s, mx * s + my * c, 1.5 * (x1 - x0).max(y1 - y0), ang);
    }
    Some(Rostro { puntos: puntos.into_iter().map(|(x, y)| (x / w, y / h)).collect() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encuentra_los_rasgos_de_los_ejemplos() {
        for e in crate::ejemplos::EJEMPLOS {
            let img = image::load_from_memory(e.datos).unwrap().to_rgb8();
            let r = detectar(&img).unwrap_or_else(|| panic!("sin rostro: {}", e.clave));
            // Ojos a la misma altura más o menos, la boca debajo de la nariz y esta debajo de los ojos.
            let (oi, od) = (r.centro(indices::OJO_I), r.centro(indices::OJO_D));
            let (nariz, boca) = (r.punto(1), r.centro(indices::LABIOS));
            assert!(oi.0 < od.0 && (oi.1 - od.1).abs() < 0.05, "{}: ojos {oi:?} {od:?}", e.clave);
            assert!(nariz.1 > oi.1 && boca.1 > nariz.1, "{}", e.clave);
            if let Ok(dir) = std::env::var("DEDALO_RASGOS") {
                let mut c = img.clone();
                for (k, &(u, v)) in r.puntos.iter().enumerate() {
                    let (x, y) = ((u * img.width() as f32) as i32, (v * img.height() as f32) as i32);
                    let col = if indices::OVALO.contains(&k) { [255, 0, 0] } else if indices::OJO_I.contains(&k) || indices::OJO_D.contains(&k) { [0, 200, 0] } else { [0, 120, 255] };
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (-1, 0), (0, -1)] {
                        let (a, b) = (x + dx, y + dy);
                        if a >= 0 && b >= 0 && (a as u32) < c.width() && (b as u32) < c.height() {
                            c.put_pixel(a as u32, b as u32, image::Rgb(col));
                        }
                    }
                }
                c.save(format!("{dir}/{}.png", e.clave)).unwrap();
            }
        }
    }
}
