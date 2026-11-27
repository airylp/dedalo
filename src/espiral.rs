//! Espiral: una sola línea que sale del centro y gira hasta cubrir el área. La
//! distancia entre vueltas y el grosor cambian con la imagen: en lo oscuro las
//! vueltas se juntan y la línea engorda; en lo claro se separan y adelgaza.
//! Se imprime terminada (no hay nada que rellenar).

use crate::flujo::Opciones;
use crate::patron::{Campo, Control};
use crate::tono::{Ajuste, Tono};
use rayon::prelude::*;
use std::f32::consts::TAU;
use std::sync::atomic::Ordering;

/// Separación entre vueltas (× periodo) en blanco y en negro.
const SEP_CLARO: f32 = 1.15;
const SEP_OSCURO: f32 = 0.7;
/// En lo más oscuro la línea ocupa como mucho esta parte del hueco entre vueltas.
const TINTA_MAX: f32 = 0.92;

/// Espiral de Arquímedes deformada sobre un área de `ancho` × `alto` mm con separación media `periodo`.
#[allow(clippy::too_many_arguments)]
pub fn espiral(ancho: f32, alto: f32, periodo: f32, tono: Option<&Tono>, ajuste: Ajuste, op: Opciones, ctl: &Control) -> Option<Campo> {
    let tono_en = |x: f32, y: f32| -> f32 {
        let t = tono.map_or(0.5, |t| if ajuste.niveles { t.nivelada(x, y) } else { t.muestra(x, y) });
        ajuste.aplicar(t)
    };
    let (cx, cy) = (ancho / 2.0, alto / 2.0);
    let r_max = cx.hypot(cy) + periodo;

    // Separación entre vueltas: del tono suavizado (≈ 1,5 periodos), para que cambie sin saltos.
    let g = periodo / 2.0;
    let (gw, gh) = ((ancho / g).ceil() as usize + 1, (alto / g).ceil() as usize + 1);
    let mut suave: Vec<f32> = (0..gw * gh).map(|k| tono_en((k % gw) as f32 * g, (k / gw) as f32 * g)).collect();
    for _ in 0..3 {
        suave = caja(&suave, gw, gh, 2);
    }
    let muestra = |x: f32, y: f32| {
        let fx = (x / g).clamp(0.0, (gw - 1) as f32);
        let fy = (y / g).clamp(0.0, (gh - 1) as f32);
        let (i, j) = ((fx as usize).min(gw - 2), (fy as usize).min(gh - 2));
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let v = |a: usize, b: usize| suave[b * gw + a];
        (v(i, j) * (1.0 - tx) + v(i + 1, j) * tx) * (1.0 - ty) + (v(i, j + 1) * (1.0 - tx) + v(i + 1, j + 1) * tx) * ty
    };
    let sep = |x: f32, y: f32| periodo * (SEP_CLARO + (SEP_OSCURO - SEP_CLARO) * muestra(x, y));

    // La línea: radios r[i] en los ángulos i·2π/M (i sigue por las vueltas: i + M es la vuelta
    // siguiente en el mismo ángulo). La primera vuelta crece del centro; cada una se aparta de
    // la anterior lo que pide la separación en ese punto.
    let m = ((TAU * r_max / (0.05 * periodo)).ceil() as usize).clamp(1024, 16384);
    let ang = |i: usize| (i % m) as f32 * TAU / m as f32;
    let punto = |r: f32, a: f32| (cx + r * a.cos(), cy + r * a.sin());
    let s0 = sep(cx, cy);
    let mut r: Vec<f32> = (0..m).map(|j| s0 * j as f32 / m as f32).collect();
    loop {
        if ctl.cancelar.load(Ordering::Relaxed) {
            return None;
        }
        let k = r.len() - m;
        let vuelta: Vec<f32> = (k..k + m)
            .map(|i| {
                let a = ang(i);
                let (x, y) = punto(r[i], a);
                // Separación a medio camino hacia la vuelta siguiente.
                let (xm, ym) = punto(r[i] + 0.5 * sep(x, y), a);
                r[i] + sep(xm, ym)
            })
            .collect();
        let min = vuelta.iter().copied().fold(f32::MAX, f32::min);
        r.extend(vuelta);
        ctl.avance.store((300.0 * (min / r_max).min(1.0)) as u32, Ordering::Relaxed);
        if min > r_max {
            break;
        }
    }
    let vueltas = r.len() / m;

    // Campo: distancia (radial) a la vuelta más cercana menos medio grosor; el grosor sale del
    // tono fino (cambia a lo largo de la línea) y del hueco entre las dos vueltas de alrededor.
    let paso = (SEP_OSCURO * periodo / 12.0).min(op.muro_min / 4.0).max(0.04);
    let (cw, ch) = ((ancho / paso).ceil() as usize + 1, (alto / paso).ceil() as usize + 1);
    let mut v = vec![0.0f32; cw * ch];
    let hechas = std::sync::atomic::AtomicUsize::new(0);
    // Radio de la vuelta k en el ángulo fraccionario `fj` (en muestras): sigue la línea entre i e i+1.
    let radio = |k: usize, fj: f32| {
        let j0 = (fj as usize).min(m - 1);
        let t = fj - j0 as f32;
        let i = k * m + j0;
        r[i] * (1.0 - t) + r[(i + 1).min(r.len() - 1)] * t
    };
    v.par_chunks_mut(cw).enumerate().for_each(|(j, fila)| {
        let y = j as f32 * paso;
        for (i, o) in fila.iter_mut().enumerate() {
            let x = i as f32 * paso;
            let (dx, dy) = (x - cx, y - cy);
            let rho = dx.hypot(dy);
            let fj = dy.atan2(dx).rem_euclid(TAU) / TAU * m as f32;
            // Primera vuelta por fuera del punto (búsqueda binaria: los radios crecen con k).
            let (mut lo, mut hi) = (0usize, vueltas - 1);
            while lo < hi {
                let mid = (lo + hi) / 2;
                if radio(mid, fj) > rho {
                    hi = mid;
                } else {
                    lo = mid + 1;
                }
            }
            let fuera = radio(lo, fj);
            let dentro = if lo > 0 { radio(lo - 1, fj) } else { 0.0 };
            let hueco = (fuera - dentro).max(1e-3);
            let d1 = (fuera - rho).min(rho - dentro).max(0.0);
            let t = tono_en(x, y);
            let medio = (t * TINTA_MAX * hueco / 2.0).max(op.muro_min / 2.0);
            let tope = ((hueco - op.canal_min) / 2.0).max(op.muro_min / 2.0).min(op.grosor_max / 2.0);
            *o = d1 - medio.min(tope);
        }
        let n = hechas.fetch_add(1, Ordering::Relaxed);
        ctl.avance.store((300 + n * 700 / ch) as u32, Ordering::Relaxed);
    });
    Some(Campo { w: cw, h: ch, v, paso, x0: 0.0, y0: 0.0, ancho, alto, solucion: vec![], recortar: true, fijo: true })
}

/// Promedio en caja de radio `r` (dos pasadas separables).
fn caja(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut t = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let (a, b) = (i.saturating_sub(r), (i + r).min(w - 1));
            t[j * w + i] = (a..=b).map(|k| v[j * w + k]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    let mut o = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let (a, b) = (j.saturating_sub(r), (j + r).min(h - 1));
            o[j * w + i] = (a..=b).map(|k| t[k * w + i]).sum::<f32>() / (b - a + 1) as f32;
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contorno::{Dibujo, Limites};
    use crate::patron::Umbrales;

    fn op() -> Opciones {
        Opciones { zonas: 1, sigue: 0.5, canal_min: 0.4, muro_min: 0.15, fondo_vacio: false, grosor_max: f32::MAX }
    }

    /// Lo oscuro lleva más tinta que lo claro, y la línea nunca se corta en el gris parejo.
    #[test]
    fn tinta_segun_el_tono() {
        let img = image::GrayImage::from_fn(200, 100, |x, _| image::Luma([if x < 100 { 230 } else { 30 }]));
        let tono = Tono::new(&img, 100.0, 50.0, 3.0);
        let c = espiral(100.0, 50.0, 3.0, Some(&tono), Ajuste::default(), op(), &Control::default()).unwrap();
        let tinta = |x0: f32, x1: f32| {
            let mut n = (0, 0);
            for j in 0..c.h {
                for i in 0..c.w {
                    let (x, y) = c.pos(i, j);
                    if x > x0 && x < x1 && (y - 25.0).abs() > 5.0 {
                        n.1 += 1;
                        n.0 += (c.at(i, j) < 0.0) as usize;
                    }
                }
            }
            n.0 as f32 / n.1 as f32
        };
        let (claro, oscuro) = (tinta(5.0, 40.0), tinta(60.0, 95.0));
        assert!(claro < 0.2 && oscuro > 0.6, "claro {claro}, oscuro {oscuro}");
    }

    /// Vista: `DEDALO_ESPIRAL=dir cargo test --release espiral_de_ejemplos -- --ignored`.
    #[test]
    #[ignore]
    fn espiral_de_ejemplos() {
        let Ok(dir) = std::env::var("DEDALO_ESPIRAL") else { return };
        let periodo: f32 = std::env::var("DEDALO_PERIODO").ok().and_then(|v| v.parse().ok()).unwrap_or(2.0);
        for e in crate::ejemplos::EJEMPLOS {
            let img = image::load_from_memory(e.datos).unwrap();
            let alto = 200.0;
            let ancho = alto * img.width() as f32 / img.height() as f32;
            let tono = Tono::new(&crate::tono::gris(&img), ancho, alto, periodo);
            let t = std::time::Instant::now();
            let c = espiral(ancho, alto, periodo, Some(&tono), Ajuste::default(), op(), &Control::default()).unwrap();
            println!("{}: {:?}", e.clave, t.elapsed());
            let u = Umbrales::new(&c);
            let d = Dibujo { campo: &c, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(periodo, 0.15, 0.4) };
            let py = 1000usize;
            let px = (py as f32 * ancho / alto) as usize;
            image::RgbaImage::from_raw(px as u32, py as u32, d.raster(px, py, None)).unwrap().save(format!("{dir}/{}.png", e.clave)).unwrap();
        }
    }
}
