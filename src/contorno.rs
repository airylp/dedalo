//! Del campo y el tono a figuras: dónde va tinta (binario) y sus contornos
//! (marching squares), listos para el PDF o el SVG.

use crate::patron::{Campo, Umbrales};
use crate::salida::{Impresion, Trazo};
use crate::tono::{Ajuste, Tono};
use rayon::prelude::*;
use std::collections::HashMap;

/// Límites del tono para que siempre quede algo que dibujar y sitio para la
/// pluma: muro mínimo y canal mínimo como fracción del periodo.
#[derive(Clone, Copy, Debug)]
pub struct Limites {
    pub tono_min: f32,
    pub tono_max: f32,
}

impl Limites {
    pub fn new(periodo: f32, muro_min: f32, canal_min: f32) -> Limites {
        let a = (muro_min / periodo).clamp(0.02, 0.9);
        let b = (1.0 - canal_min / periodo).clamp(a + 0.02, 0.98);
        Limites { tono_min: a, tono_max: b }
    }
    pub fn aplicar(&self, t: f32) -> f32 {
        self.tono_min + (self.tono_max - self.tono_min) * t
    }
}

/// Todo lo necesario para decidir si un punto lleva tinta.
pub struct Dibujo<'a> {
    pub campo: &'a Campo,
    pub umbrales: &'a Umbrales,
    pub tono: Option<&'a Tono>,
    pub ajuste: Ajuste,
    pub limites: Limites,
}

impl Dibujo<'_> {
    /// Tono final (0–1, ya con límites) en (x, y) mm.
    pub fn tono_en(&self, x: f32, y: f32) -> f32 {
        let t = self.tono.map_or(0.5, |t| if self.ajuste.niveles { t.nivelada(x, y) } else { t.muestra(x, y) });
        self.limites.aplicar(self.ajuste.aplicar(t))
    }

    /// Negativo = tinta. Fuera del área (margen del laberinto) se usa el tono del borde.
    pub fn valor(&self, x: f32, y: f32) -> f32 {
        self.campo.muestra(x, y) - self.umbrales.de(self.tono_en(x, y))
    }

    /// Rejilla de valores en las muestras del campo (para los contornos).
    fn rejilla(&self) -> Vec<f32> {
        let c = self.campo;
        let mut g = vec![0.0f32; c.w * c.h];
        g.par_chunks_mut(c.w).enumerate().for_each(|(j, fila)| {
            for (i, out) in fila.iter_mut().enumerate() {
                let (x, y) = c.pos(i, j);
                *out = c.at(i, j) - self.umbrales.de(self.tono_en(x, y));
            }
        });
        g
    }

    /// Rectángulo (mm) que ocupa el dibujo: el área, más el muro exterior del laberinto.
    pub fn extension(&self) -> (f32, f32, f32, f32) {
        let c = self.campo;
        if c.recortar { (0.0, 0.0, c.ancho, c.alto) } else { (c.x0, c.y0, c.ancho - c.x0, c.alto - c.y0) }
    }

    /// Imagen de vista previa de `px` × `py` píxeles de `extension()` (RGBA):
    /// tinta negra con bordes suavizados sobre blanco.
    #[cfg(test)]
    pub fn raster(&self, px: usize, py: usize, trazo: Option<Trazo>) -> Vec<u8> {
        self.raster_region(self.extension(), px, py, trazo)
    }

    /// Como `raster`, pero solo del rectángulo `r` (mm: x0, y0, x1, y1).
    pub fn raster_region(&self, r: (f32, f32, f32, f32), px: usize, py: usize, trazo: Option<Trazo>) -> Vec<u8> {
        let (ex0, ey0, ex1, ey1) = r;
        let (aw, ah) = (ex1 - ex0, ey1 - ey0);
        let mm = aw / px as f32;
        let mut out = vec![255u8; px * py * 4];
        out.par_chunks_mut(px * 4).enumerate().for_each(|(j, fila)| {
            let y = ey0 + (j as f32 + 0.5) * ah / py as f32;
            for i in 0..px {
                let x = ex0 + (i as f32 + 0.5) * aw / px as f32;
                let g = self.valor(x, y);
                // Distancia al borde en píxeles (por la pendiente local).
                let gx = self.valor(x + mm, y) - g;
                let gy = self.valor(x, y + mm) - g;
                let dist = g / (gx * gx + gy * gy).sqrt().max(1e-6);
                let dentro = (0.5 - dist).clamp(0.0, 1.0);
                let luz = match trazo {
                    None => 1.0 - dentro,
                    Some(t) => {
                        // Línea de `ancho` px: si es más fina que un píxel se dibuja
                        // continua y más tenue (si no, sale a trazos).
                        let ancho = t.linea / mm;
                        let borde = (ancho / 2.0 + 0.5 - dist.abs()).clamp(0.0, ancho.min(1.0));
                        let fondo = match t.impresion {
                            Impresion::Guia => 1.0 - dentro * (1.0 - t.gris),
                            Impresion::Contorno => 1.0,
                            Impresion::Relleno => 1.0 - dentro,
                        };
                        if t.impresion == Impresion::Relleno { fondo } else { fondo * (1.0 - borde) }
                    }
                };
                let c = (255.0 * luz) as u8;
                fila[i * 4..i * 4 + 3].copy_from_slice(&[c, c, c]);
            }
        });
        out
    }

    /// Contornos cerrados de las zonas con tinta (mm, recortados al área de
    /// dibujo y simplificados a `tolerancia` mm).
    pub fn contornos(&self, tolerancia: f32) -> Vec<Vec<(f32, f32)>> {
        let c = self.campo;
        let mut g = self.rejilla();
        // Fuera del área: sin tinta (las figuras se cierran en el borde).
        for j in 0..c.h {
            for i in 0..c.w {
                let (x, y) = c.pos(i, j);
                let fuera = c.recortar && (x < -1e-4 || y < -1e-4 || x > c.ancho + 1e-4 || y > c.alto + 1e-4);
                if fuera || i == 0 || j == 0 || i + 1 == c.w || j + 1 == c.h {
                    g[j * c.w + i] = g[j * c.w + i].abs().max(1e-3);
                }
            }
        }
        let lineas = marching_squares(c.w, c.h, &g);
        lineas
            .into_iter()
            .map(|l| {
                let pts: Vec<(f32, f32)> = l
                    .into_iter()
                    .map(|(x, y)| {
                        let (x, y) = (c.x0 + x * c.paso, c.y0 + y * c.paso);
                        if c.recortar { (x.clamp(0.0, c.ancho), y.clamp(0.0, c.alto)) } else { (x, y) }
                    })
                    .collect();
                simplificar(&pts, tolerancia)
            })
            .filter(|l| l.len() >= 3)
            .collect()
    }
}

/// Contornos de nivel 0 de una rejilla (negativo = dentro). Devuelve
/// polígonos cerrados (sin repetir el primer punto) en coordenadas de muestra.
pub fn marching_squares(w: usize, h: usize, g: &[f32]) -> Vec<Vec<(f32, f32)>> {
    // Cada arista de la rejilla con cruce tiene un id; los segmentos unen ids.
    // Arista horizontal (i,j)-(i+1,j): id = 2*(j*w+i); vertical (i,j)-(i,j+1): id+1.
    let at = |i: usize, j: usize| g[j * w + i];
    let cruce = |id: usize| -> (f32, f32) {
        let k = id / 2;
        let (i, j) = (k % w, k / w);
        let a = g[k];
        if id % 2 == 0 {
            let b = at(i + 1, j);
            (i as f32 + a / (a - b), j as f32)
        } else {
            let b = at(i, j + 1);
            (i as f32, j as f32 + a / (a - b))
        }
    };
    let mut sig: HashMap<usize, usize> = HashMap::new();
    for j in 0..h - 1 {
        for i in 0..w - 1 {
            let (a, b, c, d) = (at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1));
            let caso = (a < 0.0) as u8 | ((b < 0.0) as u8) << 1 | ((c < 0.0) as u8) << 2 | ((d < 0.0) as u8) << 3;
            let arriba = 2 * (j * w + i);
            let abajo = 2 * ((j + 1) * w + i);
            let izq = 2 * (j * w + i) + 1;
            let der = 2 * (j * w + i + 1) + 1;
            // Segmentos orientados (dentro a la izquierda): de → a.
            let mut seg = |p: usize, q: usize| {
                sig.insert(p, q);
            };
            match caso {
                0 | 15 => {}
                1 => seg(izq, arriba),
                2 => seg(arriba, der),
                3 => seg(izq, der),
                4 => seg(der, abajo),
                5 => {
                    if (a + b + c + d) / 4.0 < 0.0 {
                        seg(izq, abajo);
                        seg(der, arriba);
                    } else {
                        seg(izq, arriba);
                        seg(der, abajo);
                    }
                }
                6 => seg(arriba, abajo),
                7 => seg(izq, abajo),
                8 => seg(abajo, izq),
                9 => seg(abajo, arriba),
                10 => {
                    if (a + b + c + d) / 4.0 < 0.0 {
                        seg(arriba, izq);
                        seg(abajo, der);
                    } else {
                        seg(arriba, der);
                        seg(abajo, izq);
                    }
                }
                11 => seg(abajo, der),
                12 => seg(der, izq),
                13 => seg(der, arriba),
                14 => seg(arriba, izq),
                _ => unreachable!(),
            }
        }
    }
    let mut out = vec![];
    let mut inicios: Vec<usize> = sig.keys().copied().collect();
    inicios.sort_unstable();
    for inicio in inicios {
        if !sig.contains_key(&inicio) {
            continue;
        }
        let mut l = vec![];
        let mut p = inicio;
        while let Some(q) = sig.remove(&p) {
            l.push(cruce(p));
            p = q;
            if p == inicio {
                break;
            }
        }
        if l.len() >= 3 {
            out.push(l);
        }
    }
    out
}

/// Ramer–Douglas–Peucker para un polígono cerrado.
pub fn simplificar(p: &[(f32, f32)], tol: f32) -> Vec<(f32, f32)> {
    if p.len() < 8 || tol <= 0.0 {
        return p.to_vec();
    }
    // Se parte por el punto más lejano al primero.
    let lejos = (1..p.len()).max_by(|&a, &b| d2(p[0], p[a]).total_cmp(&d2(p[0], p[b]))).unwrap();
    let mut keep = vec![false; p.len()];
    keep[0] = true;
    keep[lejos] = true;
    rdp(p, 0, lejos, tol, &mut keep);
    let cola: Vec<(f32, f32)> = p[lejos..].iter().copied().chain(std::iter::once(p[0])).collect();
    let mut keep2 = vec![false; cola.len()];
    rdp(&cola, 0, cola.len() - 1, tol, &mut keep2);
    for (k, &b) in keep2.iter().enumerate().take(cola.len() - 1) {
        if b {
            keep[lejos + k] = true;
        }
    }
    p.iter().zip(keep).filter(|(_, k)| *k).map(|(q, _)| *q).collect()
}

fn d2(a: (f32, f32), b: (f32, f32)) -> f32 {
    (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)
}

fn rdp(p: &[(f32, f32)], a: usize, b: usize, tol: f32, keep: &mut [bool]) {
    if b <= a + 1 {
        return;
    }
    let (ax, ay) = p[a];
    let (bx, by) = p[b];
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let (mut peor, mut k) = (0.0f32, a);
    for (i, q) in p.iter().enumerate().take(b).skip(a + 1) {
        let d = ((q.0 - ax) * dy - (q.1 - ay) * dx).abs() / len;
        if d > peor {
            peor = d;
            k = i;
        }
    }
    if peor > tol {
        keep[k] = true;
        rdp(p, a, k, tol, keep);
        rdp(p, k, b, tol, keep);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn circulo_da_un_contorno_cerrado() {
        let (w, h) = (40, 40);
        let g: Vec<f32> = (0..w * h).map(|k| (((k % w) as f32 - 20.0).powi(2) + ((k / w) as f32 - 20.0).powi(2)).sqrt() - 10.0).collect();
        let c = marching_squares(w, h, &g);
        assert_eq!(c.len(), 1);
        for &(x, y) in &c[0] {
            let r = ((x - 20.0).powi(2) + (y - 20.0).powi(2)).sqrt();
            assert!((r - 10.0).abs() < 0.3, "radio {r}");
        }
        let s = simplificar(&c[0], 0.1);
        assert!(s.len() < c[0].len() && s.len() > 8);
    }

    #[test]
    fn anillo_da_dos_contornos() {
        let (w, h) = (60, 60);
        let g: Vec<f32> = (0..w * h).map(|k| (((((k % w) as f32 - 30.0).powi(2) + ((k / w) as f32 - 30.0).powi(2)).sqrt()) - 18.0).abs() - 5.0).collect();
        assert_eq!(marching_squares(w, h, &g).len(), 2);
    }
}

#[cfg(test)]
mod vista {
    use super::*;
    use crate::patron::{laberinto, reaccion_difusion, Control, RD_PASOS};

    /// DEDALO_PNG=carpeta: dibuja los dos patrones sobre una imagen de prueba
    /// (degradado + círculo) y guarda la vista previa y el PDF.
    #[test]
    #[ignore]
    fn vista_previa() {
        let Ok(dir) = std::env::var("DEDALO_PNG") else { return };
        let img = image::GrayImage::from_fn(300, 400, |x, y| {
            let r = ((x as f32 - 150.0).powi(2) + (y as f32 - 170.0).powi(2)).sqrt();
            let fondo = 255.0 * (1.0 - y as f32 / 400.0 * 0.6);
            image::Luma([if r < 90.0 { (r / 90.0 * 160.0) as u8 } else { fondo as u8 }])
        });
        let (ancho, alto, periodo) = (150.0, 200.0, 4.0);
        let tono = Tono::new(&img, ancho, alto, periodo);
        let ctl = Control::default();
        for (nombre, campo) in [
            ("reaccion", reaccion_difusion(ancho, alto, periodo, 1, RD_PASOS, &ctl).unwrap()),
            ("laberinto", laberinto(ancho, alto, periodo, 1, &ctl).unwrap()),
        ] {
            let u = Umbrales::new(&campo);
            let d = Dibujo { campo: &campo, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(periodo, 0.4, 1.2) };
            let (px, py) = (600, (600.0 * campo.alto / campo.ancho) as usize);
            let r = d.raster(px, py, None);
            image::RgbaImage::from_raw(px as u32, py as u32, r).unwrap().save(format!("{dir}/{nombre}.png")).unwrap();
            let t = std::time::Instant::now();
            let figs = d.contornos(0.05);
            let pag = crate::salida::Pagina::new(crate::salida::Tamano::Hoja, 15.0, 15.0, ancho / alto);
            let trazo = crate::salida::Trazo { impresion: crate::salida::Impresion::Guia, linea: 0.2, gris: 0.88 };
            let pdf = crate::salida::pdf(&pag, pag.origen(campo.ancho, campo.alto), &figs, trazo, nombre);
            println!("{nombre}: {} figuras, {} puntos, PDF {} KB, {:?}", figs.len(), figs.iter().map(Vec::len).sum::<usize>(), pdf.len() / 1024, t.elapsed());
            std::fs::write(format!("{dir}/{nombre}.pdf"), pdf).unwrap();
        }
    }
}
