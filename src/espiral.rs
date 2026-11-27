//! Espiral: una sola línea que sale del centro y gira hasta cubrir el área. La
//! distancia entre vueltas y el grosor cambian con la imagen: en lo oscuro las
//! vueltas se juntan y la línea engorda; en lo claro se separan y adelgaza.
//! Se imprime terminada (no hay nada que rellenar).

use crate::flujo::Opciones;
use crate::patron::{Campo, Control};
use crate::rostro::Cara;
use crate::tono::{Ajuste, Tono};
use rayon::prelude::*;
use std::f32::consts::TAU;
use std::sync::atomic::Ordering;

/// Ajustes propios de las espirales (se guardan en la sesión).
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AjusteEspiral {
    /// Punto de inicio en fracción del área (0–1); sin él: el centro (o la cara, en la densa).
    #[serde(rename = "origen", skip_serializing_if = "Option::is_none")]
    pub centro: Option<(f32, f32)>,
    /// Espiral: cuánto cambia la separación con el tono (0 = nada; 0,45 = de 1,23·p en blanco a 0,78·p en negro).
    #[serde(rename = "variacion")]
    pub variacion: f32,
    /// Escala (mm) a la que se suaviza el tono que decide la separación: menos = más detalle.
    #[serde(rename = "suavizado_mm")]
    pub suavizado: f32,
    /// Lo mismo en la espiral densa (más fino: ahí el tono es la densidad de vueltas).
    #[serde(rename = "suavizado_densa_mm")]
    pub suavizado_densa: f32,
    /// Densa: separación entre vueltas en blanco y en negro (mm).
    #[serde(rename = "claro_mm")]
    pub claro: f32,
    #[serde(rename = "oscuro_mm")]
    pub oscuro: f32,
    /// Densa: grosor de la línea (mm) y cuánto engorda en lo oscuro (0 = nada, 1 = el doble).
    #[serde(rename = "linea_mm")]
    pub linea: f32,
    #[serde(rename = "engorde")]
    pub engorde: f32,
    /// Densa con rostro: parar al abarcar cara, pelo y cuello (si no, toda el área).
    #[serde(rename = "solo_rostro")]
    pub solo_rostro: bool,
}

impl Default for AjusteEspiral {
    fn default() -> AjusteEspiral {
        AjusteEspiral { centro: None, variacion: 0.45, suavizado: 3.0, suavizado_densa: 1.5, claro: 8.0, oscuro: 0.8, linea: 0.35, engorde: 0.8, solo_rostro: true }
    }
}

impl AjusteEspiral {
    /// Valores dentro de lo razonable (el archivo se puede editar a mano).
    pub fn sano(mut self) -> AjusteEspiral {
        self.centro = self.centro.map(|(x, y)| (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
        self.variacion = self.variacion.clamp(0.0, 1.2);
        self.suavizado = self.suavizado.clamp(0.3, 12.0);
        self.suavizado_densa = self.suavizado_densa.clamp(0.3, 12.0);
        self.claro = self.claro.clamp(1.0, 30.0);
        self.linea = self.linea.clamp(0.1, 2.0);
        self.oscuro = self.oscuro.clamp(self.linea * 1.2, self.claro);
        self.engorde = self.engorde.clamp(0.0, 2.0);
        self
    }
}

/// Promedio en caja que suaviza a `mm` (tres pasadas de una caja de lado ≈ mm).
fn suavizar_a(v: Vec<f32>, w: usize, h: usize, g: f32, mm: f32) -> Vec<f32> {
    let r = ((mm / g) / 2.0).round() as usize;
    let mut v = v;
    if r > 0 {
        for _ in 0..3 {
            v = caja(&v, w, h, r);
        }
    }
    v
}

/// Distancia máxima del punto `c` a las esquinas del área.
fn hasta_esquinas(ancho: f32, alto: f32, c: (f32, f32)) -> f32 {
    [(0.0, 0.0), (ancho, 0.0), (0.0, alto), (ancho, alto)].iter().map(|&(x, y)| (x - c.0).hypot(y - c.1)).fold(0.0, f32::max)
}
/// En lo más oscuro la línea ocupa como mucho esta parte del hueco entre vueltas.
const TINTA_MAX: f32 = 0.92;

/// Espiral de Arquímedes deformada sobre un área de `ancho` × `alto` mm con separación media `periodo`.
#[allow(clippy::too_many_arguments)]
pub fn espiral(ancho: f32, alto: f32, periodo: f32, tono: Option<&Tono>, ajuste: Ajuste, op: Opciones, ae: &AjusteEspiral, ctl: &Control) -> Option<Campo> {
    let tono_en = |x: f32, y: f32| -> f32 {
        let t = tono.map_or(0.5, |t| if ajuste.niveles { t.nivelada(x, y) } else { t.muestra(x, y) });
        ajuste.aplicar(t)
    };
    let (cx, cy) = ae.centro.map_or((ancho / 2.0, alto / 2.0), |(u, v)| (u * ancho, v * alto));
    let r_max = hasta_esquinas(ancho, alto, (cx, cy)) + periodo;

    // Separación entre vueltas: del tono suavizado (`suavizado` mm), para que cambie sin saltos;
    // de (1 + v/2)·p en blanco a (1 − v/2)·p en negro, nunca menos de lo que cabe.
    let g = (ae.suavizado / 3.0).clamp(0.1, periodo / 2.0);
    let (gw, gh) = ((ancho / g).ceil() as usize + 1, (alto / g).ceil() as usize + 1);
    let suave = suavizar_a((0..gw * gh).map(|k| tono_en((k % gw) as f32 * g, (k / gw) as f32 * g)).collect(), gw, gh, g, ae.suavizado);
    let muestra = |x: f32, y: f32| bilineal(&suave, gw, gh, x / g, y / g);
    let sep_min = (op.muro_min + op.canal_min).max(0.2 * periodo);
    let sep = |x: f32, y: f32| (periodo * (1.0 + ae.variacion * (0.5 - muestra(x, y)))).max(sep_min);

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
    // Grosor: del tono fino (cambia a lo largo de la línea) y del hueco entre las dos vueltas de alrededor.
    let paso = (sep_min / 12.0).min(op.muro_min / 4.0).max(0.04);
    Some(campo(ancho, alto, (cx, cy), &r, m, paso, ctl, |x, y, hueco| {
        let medio = (tono_en(x, y) * TINTA_MAX * hueco / 2.0).max(op.muro_min / 2.0);
        let tope = ((hueco - op.canal_min) / 2.0).max(op.muro_min / 2.0).min(op.grosor_max / 2.0);
        medio.min(tope)
    }))
}

/// Espiral densa: una línea fina de grosor fijo que sale del centro (del rostro, si lo hay); cada
/// vuelta se aparta de la anterior según el tono: en lo oscuro casi la toca, en lo claro se va lejos.
/// El tono lo da la densidad de vueltas, así que la espiral sale irregular: se aprieta sobre ojos,
/// cejas o boca y se abre en lo blanco. Termina cuando abarca el rostro (o toda el área sin rostro).
#[allow(clippy::too_many_arguments)]
pub fn espiral_densa(ancho: f32, alto: f32, tono: Option<&Tono>, ajuste: Ajuste, op: Opciones, ae: &AjusteEspiral, cara: Option<&Cara>, ctl: &Control) -> Option<Campo> {
    let tono_en = |x: f32, y: f32| -> f32 {
        let t = tono.map_or(0.5, |t| if ajuste.niveles { t.nivelada(x, y) } else { t.muestra(x, y) });
        ajuste.aplicar(t)
    };
    let ae = ae.sano();
    let auto = cara.map_or((ancho / 2.0, alto / 2.0), |c| (c.centro.0.clamp(0.0, ancho), c.centro.1.clamp(0.0, alto)));
    let (cx, cy) = ae.centro.map_or(auto, |(u, v)| (u * ancho, v * alto));
    // Grosor de la línea y separación entre vueltas: la densidad (1/sep) va con el tono, de
    // `s_max` en blanco a `s_min` en negro.
    let w = ae.linea.min(op.grosor_max);
    let (s_min, s_max) = (ae.oscuro.max(1.2 * w), ae.claro.max(ae.oscuro.max(1.2 * w)));
    let periodo = (0.5 * (s_min + s_max)).clamp(0.5, 12.0);
    let r_max = hasta_esquinas(ancho, alto, (cx, cy)) + s_max;

    // Tono suavizado a `suavizado` mm: las vueltas siguen los rasgos sin temblar.
    let g = (ae.suavizado_densa / 3.0).clamp(0.1, 2.0);
    let (gw, gh) = ((ancho / g).ceil() as usize + 1, (alto / g).ceil() as usize + 1);
    let suave = suavizar_a((0..gw * gh).map(|k| tono_en((k % gw) as f32 * g, (k / gw) as f32 * g)).collect(), gw, gh, g, ae.suavizado_densa);
    let muestra = |x: f32, y: f32| bilineal(&suave, gw, gh, x / g, y / g);
    let sep = |x: f32, y: f32| {
        let t = muestra(x, y).clamp(0.0, 1.0);
        1.0 / (1.0 / s_max + (1.0 / s_min - 1.0 / s_max) * t)
    };

    // Hasta dónde llegar en cada ángulo: el borde del área o, con rostro (y «solo rostro»), hasta
    // salir de una elipse que abarca cara, pelo y cuello (vale aunque el origen quede fuera de ella).
    let m = ((TAU * r_max / (0.05 * periodo.max(s_min))).ceil() as usize).clamp(1024, 16384);
    let ang = |i: usize| (i % m) as f32 * TAU / m as f32;
    let punto = |r: f32, a: f32| (cx + r * a.cos(), cy + r * a.sin());
    let objetivo: Vec<f32> = (0..m)
        .map(|i| {
            let (c, s) = (ang(i).cos(), ang(i).sin());
            // Distancia al borde del área por el rayo.
            let borde = |d: f32, p: f32, lim: f32| if d > 1e-6 { (lim - p) / d } else if d < -1e-6 { -p / d } else { f32::MAX };
            let al_borde = borde(c, cx, ancho).min(borde(s, cy, alto)).max(0.0);
            match cara.filter(|_| ae.solo_rostro) {
                Some(f) => {
                    let (ax, ay) = (1.5 * f.semi.0, 1.35 * f.semi.1);
                    // Rayo o + t·d contra la elipse: la salida más lejana (0 si no la toca).
                    let (ox, oy) = ((cx - f.centro.0) / ax, (cy - f.centro.1) / ay);
                    let (dx, dy) = (c / ax, s / ay);
                    let (a, b, k) = (dx * dx + dy * dy, 2.0 * (ox * dx + oy * dy), ox * ox + oy * oy - 1.0);
                    let disc = b * b - 4.0 * a * k;
                    let salida = if disc > 0.0 { ((-b + disc.sqrt()) / (2.0 * a)).max(0.0) } else { 0.0 };
                    salida.min(al_borde)
                }
                None => al_borde,
            }
        })
        .collect();

    let mut r: Vec<f32> = (0..m).map(|j| s_min * j as f32 / m as f32).collect();
    let mut inc = vec![0.0f32; m];
    loop {
        if ctl.cancelar.load(Ordering::Relaxed) {
            return None;
        }
        let k = r.len() - m;
        for (j, d) in inc.iter_mut().enumerate() {
            let (i, a) = (k + j, ang(k + j));
            // Separación medida a medio camino hacia la vuelta siguiente (dos tanteos).
            let mut s = sep(punto(r[i], a).0, punto(r[i], a).1);
            for _ in 0..2 {
                let (xm, ym) = punto(r[i] + 0.5 * s, a);
                s = sep(xm, ym);
            }
            *d = s;
        }
        let vuelta: Vec<f32> = (0..m).map(|j| r[k + j] + inc[j]).collect();
        let hecho = vuelta.iter().zip(&objetivo).all(|(v, o)| v >= o);
        let falta = vuelta.iter().zip(&objetivo).map(|(v, o)| v / o).fold(f32::MAX, f32::min);
        r.extend(vuelta);
        ctl.avance.store((300.0 * falta.min(1.0)) as u32, Ordering::Relaxed);
        if hecho || r.len() / m > 4000 {
            break;
        }
    }
    // Suavizado a lo largo de la línea (≈ medio `suavizado` de arco), por tramos de una vuelta: curvas
    // sin dientes ni costura. Es lineal, así que entre dos vueltas queda la media de los huecos
    // (nunca menos de `s_min`); el tope de abajo solo corrige los cambios de ventana.
    let crudo = r.clone();
    let mut pre = vec![0.0f64; crudo.len() + 1];
    for (i, v) in crudo.iter().enumerate() {
        pre[i + 1] = pre[i] + *v as f64;
    }
    for k in 1..r.len() / m {
        let r_med = crudo[k * m..(k + 1) * m].iter().sum::<f32>() / m as f32;
        let ventana = ((ae.suavizado_densa / (r_med.max(s_min) * TAU / m as f32)) as usize).clamp(1, m / 16);
        for i in k * m..(k + 1) * m {
            let (a, b) = (i.saturating_sub(ventana), (i + ventana + 1).min(crudo.len()));
            r[i] = ((pre[b] - pre[a]) / (b - a) as f64) as f32;
            r[i] = r[i].max(r[i - m] + 0.9 * s_min);
        }
    }
    // Grosor: fijo, y algo más en lo oscuro (sin tapar el hueco entre vueltas).
    let paso = (s_min / 8.0).min(w / 3.0).max(0.04);
    let engorde = ae.engorde;
    Some(campo(ancho, alto, (cx, cy), &r, m, paso, ctl, |x, y, hueco| {
        let t = tono_en(x, y).clamp(0.0, 1.0);
        (w / 2.0 * (1.0 + engorde * t)).min(((hueco - 0.6 * w) / 2.0).max(w / 2.0)).min(op.grosor_max / 2.0)
    }))
}

fn bilineal(v: &[f32], w: usize, h: usize, fx: f32, fy: f32) -> f32 {
    let fx = fx.clamp(0.0, (w - 1) as f32);
    let fy = fy.clamp(0.0, (h - 1) as f32);
    let (i, j) = ((fx as usize).min(w - 2), (fy as usize).min(h - 2));
    let (tx, ty) = (fx - i as f32, fy - j as f32);
    let a = |a: usize, b: usize| v[b * w + a];
    (a(i, j) * (1.0 - tx) + a(i + 1, j) * tx) * (1.0 - ty) + (a(i, j + 1) * (1.0 - tx) + a(i + 1, j + 1) * tx) * ty
}

/// Campo de una espiral dada por sus radios `r` (M = `m` muestras por vuelta, centro `c`): distancia
/// radial a la vuelta más cercana menos medio grosor (`medio(x, y, hueco entre vueltas)`).
#[allow(clippy::too_many_arguments)]
fn campo(ancho: f32, alto: f32, c: (f32, f32), r: &[f32], m: usize, paso: f32, ctl: &Control, medio: impl Fn(f32, f32, f32) -> f32 + Sync) -> Campo {
    let vueltas = r.len() / m;
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
            let (dx, dy) = (x - c.0, y - c.1);
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
            if fuera <= rho {
                // Más allá de la última vuelta: en blanco.
                *o = rho - fuera - medio(x, y, 2.0 * paso);
                continue;
            }
            let dentro = if lo > 0 { radio(lo - 1, fj) } else { 0.0 };
            let hueco = (fuera - dentro).max(1e-3);
            let d1 = (fuera - rho).min(rho - dentro).max(0.0);
            *o = d1 - medio(x, y, hueco);
        }
        let n = hechas.fetch_add(1, Ordering::Relaxed);
        ctl.avance.store((300 + n * 700 / ch) as u32, Ordering::Relaxed);
    });
    Campo { w: cw, h: ch, v, paso, x0: 0.0, y0: 0.0, ancho, alto, solucion: vec![], recortar: true, fijo: true }
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
        let c = espiral(100.0, 50.0, 3.0, Some(&tono), Ajuste::default(), op(), &AjusteEspiral::default(), &Control::default()).unwrap();
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

    /// Espiral densa: en lo oscuro, más vueltas (más tinta) que en lo claro; sin rostro cubre todo.
    #[test]
    fn densa_se_aprieta_en_lo_oscuro() {
        let img = image::GrayImage::from_fn(200, 100, |x, _| image::Luma([if x < 100 { 230 } else { 30 }]));
        let tono = Tono::new(&img, 100.0, 50.0, 3.0);
        let c = espiral_densa(100.0, 50.0, Some(&tono), Ajuste::default(), op(), &AjusteEspiral::default(), None, &Control::default()).unwrap();
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
        let (claro, oscuro, esquina) = (tinta(5.0, 40.0), tinta(60.0, 95.0), tinta(0.0, 4.0));
        assert!(claro < 0.15 && oscuro > 0.35 && esquina > 0.0, "claro {claro}, oscuro {oscuro}, esquina {esquina}");
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
            // DEDALO_ORIGEN="u,v": origen movido (fracción del área).
            let mut ae = AjusteEspiral::default();
            ae.centro = std::env::var("DEDALO_ORIGEN").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?))));
            let ae = &ae;
            // DEDALO_DENSA=1: la espiral densa (centrada en el rostro).
            let c = if std::env::var("DEDALO_DENSA").is_ok() {
                let cara = crate::rostro::detectar(&img.to_rgb8()).map(|r| Cara::new(&r, ancho, alto));
                espiral_densa(ancho, alto, Some(&tono), Ajuste::default(), op(), ae, cara.as_ref(), &Control::default()).unwrap()
            } else {
                espiral(ancho, alto, periodo, Some(&tono), Ajuste::default(), op(), ae, &Control::default()).unwrap()
            };
            println!("{}: {:?}", e.clave, t.elapsed());
            let u = Umbrales::new(&c);
            let d = Dibujo { campo: &c, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(periodo, 0.15, 0.4) };
            let py = 1000usize;
            let px = (py as f32 * ancho / alto) as usize;
            image::RgbaImage::from_raw(px as u32, py as u32, d.raster(px, py, None)).unwrap().save(format!("{dir}/{}.png", e.clave)).unwrap();
        }
    }
}
