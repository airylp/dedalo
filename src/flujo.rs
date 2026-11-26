//! Líneas de flujo: líneas que avanzan con curvas suaves sin cruzarse nunca;
//! en lo oscuro se juntan (y luego engordan con el umbral), en lo claro se
//! separan.
//!
//! - Dirección: ruido suave (valor por retícula) mezclado con el «Edge Tangent
//!   Flow» de la imagen (tensor de estructura: las líneas rodean las formas).
//! - Colocación: Jobard y Lefer (1997), «Creating Evenly-Spaced Streamlines of
//!   Arbitrary Density», con la separación variable según el tono.
//! - Zonas: bandas de tono (suavizado); cada zona gira su dirección y sus
//!   líneas se detienen en el borde, así cada zona se rellena con las suyas.
//!
//! El campo resultante es la distancia a la línea más cercana dividida por la
//! separación local: con el umbral por cuantiles, el grosor sigue al tono.

use crate::patron::{Azar, Campo, Control};
use crate::tono::{Ajuste, Tono};
use rayon::prelude::*;
use std::collections::VecDeque;
use std::sync::atomic::Ordering;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Opciones {
    /// Bandas de tono (1 = sin zonas).
    pub zonas: u32,
    /// 0 = solo ruido, 1 = todo lo posible siguiendo los bordes de la imagen.
    pub sigue: f32,
    /// Ancho mínimo del blanco entre dos líneas (mm).
    pub canal_min: f32,
    /// Grosor mínimo de una línea (mm).
    pub muro_min: f32,
}

/// Rejilla gruesa de valores (tono, zona, dirección).
struct Rejilla {
    w: usize,
    h: usize,
    paso: f32,
    v: Vec<f32>,
}

impl Rejilla {
    fn new(ancho: f32, alto: f32, paso: f32, f: impl Fn(f32, f32) -> f32 + Sync) -> Rejilla {
        let w = (ancho / paso).ceil() as usize + 1;
        let h = (alto / paso).ceil() as usize + 1;
        let mut v = vec![0.0; w * h];
        v.par_chunks_mut(w).enumerate().for_each(|(j, fila)| {
            for (i, o) in fila.iter_mut().enumerate() {
                *o = f(i as f32 * paso, j as f32 * paso);
            }
        });
        Rejilla { w, h, paso, v }
    }

    fn at(&self, i: usize, j: usize) -> f32 {
        self.v[j.min(self.h - 1) * self.w + i.min(self.w - 1)]
    }

    fn muestra(&self, x: f32, y: f32) -> f32 {
        let fx = (x / self.paso).clamp(0.0, (self.w - 1) as f32);
        let fy = (y / self.paso).clamp(0.0, (self.h - 1) as f32);
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let a = self.at(i, j) * (1.0 - tx) + self.at(i + 1, j) * tx;
        let b = self.at(i, j + 1) * (1.0 - tx) + self.at(i + 1, j + 1) * tx;
        a * (1.0 - ty) + b * ty
    }

    fn cercano(&self, x: f32, y: f32) -> f32 {
        let i = (x / self.paso).round().clamp(0.0, (self.w - 1) as f32) as usize;
        let j = (y / self.paso).round().clamp(0.0, (self.h - 1) as f32) as usize;
        self.at(i, j)
    }

    /// Promedio en una caja de radio `r` celdas (dos pasadas separables).
    fn suavizar(&mut self, r: usize) {
        if r == 0 {
            return;
        }
        let (w, h) = (self.w, self.h);
        let mut t = vec![0.0f32; w * h];
        for j in 0..h {
            for i in 0..w {
                let (a, b) = (i.saturating_sub(r), (i + r).min(w - 1));
                t[j * w + i] = (a..=b).map(|k| self.v[j * w + k]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
        for j in 0..h {
            for i in 0..w {
                let (a, b) = (j.saturating_sub(r), (j + r).min(h - 1));
                self.v[j * w + i] = (a..=b).map(|k| t[k * w + i]).sum::<f32>() / (b - a + 1) as f32;
            }
        }
    }
}

/// Ruido suave de direcciones: vectores al azar en una retícula, interpolados.
struct Ruido {
    n: usize,
    lado: f32,
    vec: Vec<(f32, f32)>,
}

impl Ruido {
    fn new(semilla: u64, lado: f32, ancho: f32, alto: f32) -> Ruido {
        let n = ((ancho.max(alto) / lado).ceil() as usize + 3).max(4);
        let mut az = Azar::new(semilla ^ 0xF10);
        let vec = (0..n * n)
            .map(|_| {
                let a = az.real() * std::f32::consts::TAU;
                (a.cos(), a.sin())
            })
            .collect();
        Ruido { n, lado, vec }
    }

    /// Ángulo en (x, y) mm.
    fn angulo(&self, x: f32, y: f32) -> f32 {
        let (fx, fy) = (x / self.lado + 1.0, y / self.lado + 1.0);
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (i, j) = (i.min(self.n - 2), j.min(self.n - 2));
        let s = |t: f32| t * t * (3.0 - 2.0 * t);
        let (tx, ty) = (s((fx - i as f32).clamp(0.0, 1.0)), s((fy - j as f32).clamp(0.0, 1.0)));
        let g = |a: usize, b: usize| self.vec[b * self.n + a];
        let mix = |p: (f32, f32), q: (f32, f32), t: f32| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
        let v = mix(mix(g(i, j), g(i + 1, j), tx), mix(g(i, j + 1), g(i + 1, j + 1), tx), ty);
        v.1.atan2(v.0)
    }
}

/// Punto de una línea en la rejilla espacial.
#[derive(Clone, Copy)]
struct Punto {
    x: f32,
    y: f32,
    linea: u32,
    idx: i32,
}

/// Rejilla espacial de puntos (celdas de `lado` mm).
struct Espacio {
    w: usize,
    h: usize,
    lado: f32,
    celdas: Vec<Vec<Punto>>,
}

impl Espacio {
    fn new(ancho: f32, alto: f32, lado: f32) -> Espacio {
        let w = (ancho / lado).ceil() as usize + 1;
        let h = (alto / lado).ceil() as usize + 1;
        Espacio { w, h, lado, celdas: vec![vec![]; w * h] }
    }
    fn celda(&self, x: f32, y: f32) -> (usize, usize) {
        (((x / self.lado).max(0.0) as usize).min(self.w - 1), ((y / self.lado).max(0.0) as usize).min(self.h - 1))
    }
    fn poner(&mut self, p: Punto) {
        let (i, j) = self.celda(p.x, p.y);
        self.celdas[j * self.w + i].push(p);
    }
    fn quitar_linea(&mut self, linea: u32) {
        for c in &mut self.celdas {
            c.retain(|p| p.linea != linea);
        }
    }
    /// ¿Hay algún punto a menos de `d` de (x, y)? (sin contar los de la misma
    /// línea cuyo índice esté a menos de `salto` de `idx`).
    fn ocupado(&self, x: f32, y: f32, d: f32, linea: u32, idx: i32, salto: i32) -> bool {
        let (ci, cj) = self.celda(x, y);
        let r = (d / self.lado).ceil() as usize;
        let d2 = d * d;
        for j in cj.saturating_sub(r)..=(cj + r).min(self.h - 1) {
            for i in ci.saturating_sub(r)..=(ci + r).min(self.w - 1) {
                for p in &self.celdas[j * self.w + i] {
                    if p.linea == linea && (p.idx - idx).abs() < salto {
                        continue;
                    }
                    if (p.x - x).powi(2) + (p.y - y).powi(2) < d2 {
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Líneas de flujo sobre un área de `ancho` × `alto` mm con separación media `periodo`.
#[allow(clippy::too_many_arguments)]
pub fn flujo(ancho: f32, alto: f32, periodo: f32, semilla: u64, tono: Option<&Tono>, ajuste: Ajuste, op: Opciones, ctl: &Control) -> Option<Campo> {
    let tono_en = |x: f32, y: f32| -> f32 {
        let t = tono.map_or(0.5, |t| if ajuste.niveles { t.nivelada(x, y) } else { t.muestra(x, y) });
        ajuste.aplicar(t)
    };
    // Separación: 1,6 × periodo en blanco, 0,5 × en negro.
    let (s_max, s_min) = (1.6 * periodo, 0.5 * periodo);
    let g = periodo / 2.0;
    let mut tonos = Rejilla::new(ancho, alto, g, tono_en);
    tonos.suavizar(1);
    let sep = |x: f32, y: f32| s_max - (s_max - s_min) * tonos.muestra(x, y);

    // Zonas: bandas de tono muy suavizado.
    let zonas_n = op.zonas.clamp(1, 8);
    let mut suave = Rejilla { w: tonos.w, h: tonos.h, paso: g, v: tonos.v.clone() };
    suave.suavizar(4);
    suave.suavizar(4);
    let zona_en = |x: f32, y: f32| -> u32 { ((suave.cercano(x, y) * zonas_n as f32) as u32).min(zonas_n - 1) };

    // Dirección: ruido + tangente de los bordes (tensor de estructura).
    let ruido = Ruido::new(semilla, 12.0 * periodo, ancho, alto);
    let (w, h) = (tonos.w, tonos.h);
    let (mut jxx, mut jxy, mut jyy) = (vec![0.0f32; w * h], vec![0.0f32; w * h], vec![0.0f32; w * h]);
    for j in 0..h {
        for i in 0..w {
            let gx = tonos.at(i + 1, j) - tonos.at(i.saturating_sub(1), j);
            let gy = tonos.at(i, j + 1) - tonos.at(i, j.saturating_sub(1));
            jxx[j * w + i] = gx * gx;
            jxy[j * w + i] = gx * gy;
            jyy[j * w + i] = gy * gy;
        }
    }
    let mut rx = Rejilla { w, h, paso: g, v: jxx };
    let mut rxy = Rejilla { w, h, paso: g, v: jxy };
    let mut ry = Rejilla { w, h, paso: g, v: jyy };
    for r in [&mut rx, &mut rxy, &mut ry] {
        r.suavizar(3);
    }
    // Fuerza del borde normalizada por un valor alto (percentil 95).
    let mut fuerzas: Vec<f32> = (0..w * h).map(|k| ((rx.v[k] - ry.v[k]).powi(2) + 4.0 * rxy.v[k].powi(2)).sqrt()).collect();
    let mut orden = fuerzas.clone();
    orden.sort_by(f32::total_cmp);
    let alta = orden[(orden.len() as f32 * 0.95) as usize].max(1e-6);
    for f in &mut fuerzas {
        *f = (*f / alta).min(1.0);
    }
    // Componentes del ángulo doble (las direcciones no tienen sentido: 0° = 180°).
    let mut cx = vec![0.0f32; w * h];
    let mut cy = vec![0.0f32; w * h];
    for j in 0..h {
        for i in 0..w {
            let k = j * w + i;
            let (x, y) = (i as f32 * g, j as f32 * g);
            let zona = zona_en(x, y) as f32;
            let a_ruido = ruido.angulo(x, y) + zona * 1.0;
            // Dirección del gradiente: 0.5·atan2(2Jxy, Jxx−Jyy); la tangente va a 90°.
            let a_borde = 0.5 * (2.0 * rxy.v[k]).atan2(rx.v[k] - ry.v[k]) + std::f32::consts::FRAC_PI_2;
            let pb = op.sigue * fuerzas[k];
            cx[k] = (1.0 - pb) * (2.0 * a_ruido).cos() + pb * (2.0 * a_borde).cos();
            cy[k] = (1.0 - pb) * (2.0 * a_ruido).sin() + pb * (2.0 * a_borde).sin();
        }
    }
    let dir_x = Rejilla { w, h, paso: g, v: cx };
    let dir_y = Rejilla { w, h, paso: g, v: cy };
    let direccion = |x: f32, y: f32| -> (f32, f32) {
        let a = 0.5 * dir_y.muestra(x, y).atan2(dir_x.muestra(x, y));
        (a.cos(), a.sin())
    };

    // ---- Jobard–Lefer
    let paso_l = 0.1 * periodo;
    // Dos líneas nunca se acercan más de lo que cabe: un canal y dos medios muros… y algo de holgura.
    let cabe = 1.15 * (op.canal_min + op.muro_min);
    let mut espacio = Espacio::new(ancho, alto, s_max);
    let mut lineas: Vec<Vec<(f32, f32)>> = vec![];
    let dentro = |x: f32, y: f32| x >= 0.0 && y >= 0.0 && x <= ancho && y <= alto;
    let mut az = Azar::new(semilla);
    // Semillas de reserva: rejilla con algo de azar, en orden aleatorio.
    let mut reserva: Vec<(f32, f32)> = vec![];
    let rs = 0.5 * s_min;
    let (nx, ny) = ((ancho / rs) as usize + 1, (alto / rs) as usize + 1);
    for j in 0..ny {
        for i in 0..nx {
            reserva.push(((i as f32 + az.real()) * rs, (j as f32 + az.real()) * rs));
        }
    }
    for k in (1..reserva.len()).rev() {
        let o = az.hasta(k + 1);
        reserva.swap(k, o);
    }
    let total = reserva.len();
    let mut cola: VecDeque<(f32, f32)> = VecDeque::new();
    let mut siguiente_reserva = 0usize;
    let max_pasos = ((ancho + alto) * 4.0 / paso_l) as usize;
    loop {
        if ctl.cancelar.load(Ordering::Relaxed) {
            return None;
        }
        let semilla_p = if let Some(p) = cola.pop_front() {
            p
        } else if siguiente_reserva < total {
            siguiente_reserva += 1;
            ctl.avance.store((siguiente_reserva * 800 / total) as u32, Ordering::Relaxed);
            reserva[siguiente_reserva - 1]
        } else {
            break;
        };
        let (sx, sy) = semilla_p;
        if !dentro(sx, sy) || espacio.ocupado(sx, sy, 0.95 * sep(sx, sy), u32::MAX, 0, 0) {
            continue;
        }
        let id = lineas.len() as u32;
        let zona = zona_en(sx, sy);
        let salto = (2.0 * s_max / paso_l).ceil() as i32;
        let mut mitades: [Vec<(f32, f32)>; 2] = [vec![], vec![]];
        espacio.poner(Punto { x: sx, y: sy, linea: id, idx: 0 });
        for (lado, signo) in [(0usize, 1.0f32), (1, -1.0)] {
            let (mut x, mut y) = (sx, sy);
            let (dx0, dy0) = direccion(x, y);
            let mut prev = (dx0 * signo, dy0 * signo);
            for n in 1..=max_pasos {
                // RK2 (punto medio), manteniendo el sentido.
                let orienta = |d: (f32, f32), p: (f32, f32)| if d.0 * p.0 + d.1 * p.1 < 0.0 { (-d.0, -d.1) } else { d };
                let d1 = orienta(direccion(x, y), prev);
                let (mx, my) = (x + d1.0 * paso_l * 0.5, y + d1.1 * paso_l * 0.5);
                let d2 = orienta(direccion(mx, my), d1);
                let (nx, ny) = (x + d2.0 * paso_l, y + d2.1 * paso_l);
                if !dentro(nx, ny) || zona_en(nx, ny) != zona {
                    break;
                }
                let idx = if lado == 0 { n as i32 } else { -(n as i32) };
                if espacio.ocupado(nx, ny, (0.6 * sep(nx, ny)).max(cabe), id, idx, salto) {
                    break;
                }
                espacio.poner(Punto { x: nx, y: ny, linea: id, idx });
                mitades[lado].push((nx, ny));
                prev = d2;
                x = nx;
                y = ny;
            }
        }
        let [adelante, atras] = mitades;
        let largo = (adelante.len() + atras.len()) as f32 * paso_l;
        if largo < 1.2 * sep(sx, sy) {
            espacio.quitar_linea(id);
            continue;
        }
        let mut linea: Vec<(f32, f32)> = atras.into_iter().rev().collect();
        linea.push((sx, sy));
        linea.extend(adelante);
        // Semillas a los lados, cada media separación a lo largo.
        let cada = ((0.5 * sep(sx, sy)) / paso_l).max(1.0) as usize;
        for k in (0..linea.len()).step_by(cada) {
            let (px, py) = linea[k];
            let (a, b) = (linea[k.saturating_sub(1)], linea[(k + 1).min(linea.len() - 1)]);
            let (tx, ty) = (b.0 - a.0, b.1 - a.1);
            let l = (tx * tx + ty * ty).sqrt().max(1e-6);
            let s = sep(px, py);
            cola.push_back((px - ty / l * s, py + tx / l * s));
            cola.push_back((px + ty / l * s, py - tx / l * s));
        }
        lineas.push(linea);
    }

    // ---- Campo: distancia a la línea más cercana / separación local.
    let paso = s_min / 8.0;
    let salto_campo = (2.0 * s_max / paso_l).ceil() as i32;
    let mut seg = Espacio::new(ancho, alto, s_max);
    for (id, l) in lineas.iter().enumerate() {
        for (k, &(x, y)) in l.iter().enumerate() {
            seg.poner(Punto { x, y, linea: id as u32, idx: k as i32 });
        }
    }
    let (cw, ch) = ((ancho / paso).ceil() as usize + 1, (alto / paso).ceil() as usize + 1);
    let mut v = vec![1.0f32; cw * ch];
    let hechas = std::sync::atomic::AtomicUsize::new(0);
    v.par_chunks_mut(cw).enumerate().for_each(|(j, fila)| {
        let y = j as f32 * paso;
        for (i, o) in fila.iter_mut().enumerate() {
            let x = i as f32 * paso;
            let s = sep(x, y);
            let (ci, cj) = seg.celda(x, y);
            // La línea más cercana (d1) y la más cercana de las demás (d2): el
            // campo es la posición dentro del canal entre las dos (0 = sobre la
            // línea, 0,5 = en medio). Así la tinta nunca cierra un canal.
            let (mut d1, mut id1, mut idx1) = (f32::MAX, u32::MAX, 0i32);
            let mut d2 = f32::MAX;
            for b in cj.saturating_sub(1)..=(cj + 1).min(seg.h - 1) {
                for a in ci.saturating_sub(1)..=(ci + 1).min(seg.w - 1) {
                    for p in &seg.celdas[b * seg.w + a] {
                        // Segmento p → siguiente punto de su línea.
                        let l = &lineas[p.linea as usize];
                        let k = p.idx as usize;
                        let q = l[(k + 1).min(l.len() - 1)];
                        let d = dist2_seg(x, y, p.x, p.y, q.0, q.1);
                        let misma = p.linea == id1 && (p.idx - idx1).abs() < salto_campo;
                        if d < d1 {
                            if !misma {
                                d2 = d2.min(d1);
                            }
                            (d1, id1, idx1) = (d, p.linea, p.idx);
                        } else if !misma {
                            d2 = d2.min(d);
                        }
                    }
                }
            }
            let (d1, d2) = (d1.sqrt().min(2.0 * s), d2.sqrt().min(2.0 * s));
            // Cerca del centro de un canal estrecho se fuerza el blanco: el canal
            // nunca baja de `canal_min` (sin esto quedan hilos de puntos blancos).
            let (medio, c) = ((d2 - d1) / 2.0, op.canal_min / 2.0);
            // El centro de la línea (medio muro mínimo) nunca se toca.
            let libre = if c > 0.0 && d1 > op.muro_min / 2.0 { 0.5 * (1.0 - (medio - c) / c).clamp(0.0, 1.0) } else { 0.0 };
            *o = (d1 / (d1 + d2).max(1e-6)).max(libre);
        }
        let n = hechas.fetch_add(1, Ordering::Relaxed);
        ctl.avance.store((800 + n * 200 / ch) as u32, Ordering::Relaxed);
    });
    Some(Campo { w: cw, h: ch, v, paso, x0: 0.0, y0: 0.0, ancho, alto, solucion: vec![], recortar: true })
}

fn dist2_seg(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (ax + t * dx - px, ay + t * dy - py);
    qx * qx + qy * qy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lineas_mas_juntas_en_lo_oscuro() {
        // Mitad izquierda blanca, mitad derecha negra.
        let img = image::GrayImage::from_fn(200, 100, |x, _| image::Luma([if x < 100 { 255 } else { 0 }]));
        let tono = Tono::new(&img, 100.0, 50.0, 4.0);
        let ajuste = Ajuste { niveles: false, ..Default::default() };
        let c = flujo(100.0, 50.0, 4.0, 3, Some(&tono), ajuste, Opciones { zonas: 1, sigue: 0.0, canal_min: 1.0, muro_min: 0.4 }, &Control::default()).unwrap();
        // Fracción de puntos muy cerca de una línea (campo < 0.15) a cada lado.
        let frac = |x0: f32, x1: f32| {
            let (mut n, mut cerca) = (0, 0);
            for j in 0..c.h {
                for i in 0..c.w {
                    let (x, _) = c.pos(i, j);
                    if x > x0 && x < x1 {
                        n += 1;
                        if c.at(i, j) < 0.15 {
                            cerca += 1;
                        }
                    }
                }
            }
            cerca as f32 / n as f32
        };
        // El campo está normalizado por la separación: la fracción es parecida a
        // ambos lados; lo que cambia es cuántas líneas hay. Se cuentan cruces.
        let cruces = |y: f32, x0: f32, x1: f32| {
            let mut n = 0;
            let mut antes = c.muestra(x0, y) < 0.15;
            let mut x = x0;
            while x < x1 {
                let ahora = c.muestra(x, y) < 0.15;
                if ahora && !antes {
                    n += 1;
                }
                antes = ahora;
                x += 0.1;
            }
            n
        };
        let (claro, oscuro): (usize, usize) = (5..45).step_by(5).map(|y| (cruces(y as f32, 5.0, 45.0), cruces(y as f32, 55.0, 95.0))).fold((0, 0), |a, b| (a.0 + b.0, a.1 + b.1));
        assert!(frac(5.0, 45.0) > 0.05 && frac(55.0, 95.0) > 0.05);
        assert!(oscuro as f32 > claro as f32 * 1.5, "claro {claro} oscuro {oscuro}");
    }
}
