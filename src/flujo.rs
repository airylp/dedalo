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
    /// Sin líneas en las zonas lisas y claras (fondo blanco o de un solo color claro).
    pub fondo_vacio: bool,
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
    // Separación: 1,15 × periodo en blanco, 0,55 × en negro. Varía poco: el tono
    // lo da sobre todo el grosor (en lo claro, líneas finas en vez de huecos).
    // Separación entre centros según el tono: algo más juntas en lo claro (líneas
    // finas) y más separadas en lo oscuro, donde engordan hasta dejar solo el canal
    // mínimo; así el rango de tinta es mayor (si se juntaran en lo oscuro, cada canal
    // extra sería blanco que no se puede rellenar). Pruebas: DEDALO_SEP="claro,oscuro".
    let (k_claro, k_oscuro) = std::env::var("DEDALO_SEP").ok().and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))).unwrap_or((0.85, 1.35));
    let (s_max, s_min) = (k_claro.max(k_oscuro) * periodo, k_claro.min(k_oscuro) * periodo);
    let g = periodo / 2.0;
    let mut tonos = Rejilla::new(ancho, alto, g, tono_en);
    tonos.suavizar(1);
    let sep = |x: f32, y: f32| k_claro * periodo + (k_oscuro - k_claro) * periodo * tonos.muestra(x, y);

    // Zonas: bandas de tono muy suavizado.
    let zonas_n = op.zonas.clamp(1, 8);
    let mut suave = Rejilla { w: tonos.w, h: tonos.h, paso: g, v: tonos.v.clone() };
    suave.suavizar(4);
    suave.suavizar(4);
    let zona_en = |x: f32, y: f32| -> u32 { ((suave.cercano(x, y) * zonas_n as f32) as u32).min(zonas_n - 1) };

    // Fondo liso y claro: tono bajo y casi sin variación alrededor (≈ 1,5 periodos).
    // Se limpia la máscara para no dejar islas pequeñas ni agujeros sueltos.
    let fondo = if op.fondo_vacio {
        let mut media = Rejilla { w: tonos.w, h: tonos.h, paso: g, v: tonos.v.clone() };
        let mut cuad = Rejilla { w: tonos.w, h: tonos.h, paso: g, v: tonos.v.iter().map(|t| t * t).collect() };
        media.suavizar(3);
        cuad.suavizar(3);
        let v: Vec<f32> = media
            .v
            .iter()
            .zip(&cuad.v)
            .map(|(m, q)| {
                let var = (q - m * m).max(0.0);
                ((*m < 0.3) && var < 0.0006) as u8 as f32
            })
            .collect();
        let mut r = Rejilla { w: tonos.w, h: tonos.h, paso: g, v };
        r.suavizar(2);
        Some(r)
    } else {
        None
    };
    let en_fondo = |x: f32, y: f32| fondo.as_ref().is_some_and(|f| f.muestra(x, y) > 0.6);

    // Dirección (como en el grabado): las líneas corren paralelas a los bordes de
    // la imagen (tensor de estructura a dos escalas: el detalle y la forma
    // grande, que lleva la orientación de los bordes a las zonas lisas). Donde la
    // imagen no dice nada, un ángulo base con una ondulación suave (y girado en
    // cada zona). Todo se suma como tensores: no hay saltos.
    let ruido = Ruido::new(semilla, 20.0 * periodo, ancho, alto);
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
    let tensor = |r: usize| {
        let mut a = Rejilla { w, h, paso: g, v: jxx.clone() };
        let mut b = Rejilla { w, h, paso: g, v: jxy.clone() };
        let mut c = Rejilla { w, h, paso: g, v: jyy.clone() };
        for x in [&mut a, &mut b, &mut c] {
            x.suavizar(r);
            x.suavizar(r);
        }
        // Normalizado por la «traza» alta (percentil 95) para comparar escalas.
        let mut tr: Vec<f32> = a.v.iter().zip(&c.v).map(|(p, q)| p + q).collect();
        tr.sort_by(f32::total_cmp);
        let alta = tr[(tr.len() as f32 * 0.95) as usize].max(1e-9);
        for x in [&mut a, &mut b, &mut c] {
            for v in &mut x.v {
                *v /= alta;
            }
        }
        (a, b, c)
    };
    // Escalas: del detalle (ojos, boca) a la forma grande (cabeza, hombros). Donde
    // la imagen es lisa mandan las escalas grandes: las líneas rodean la forma más
    // cercana como curvas de nivel, en vez de cruzar en diagonal.
    let escalas: Vec<(f32, (Rejilla, Rejilla, Rejilla))> = escalas_tensor().into_iter().map(|(r, p)| (p, tensor(r))).collect();
    let mut cx = vec![0.0f32; w * h];
    let mut cy = vec![0.0f32; w * h];
    let base_ang = 0.55f32;
    // Ondulación de espagueti: la dirección gira un poco con un ruido de escala media.
    let ondas = Ruido::new(semilla ^ 0x5A, 7.0 * periodo, ancho, alto);
    let amplitud = 0.35 * (1.0 - 0.5 * op.sigue);
    for j in 0..h {
        for i in 0..w {
            let k = j * w + i;
            let (x, y) = (i as f32 * g, j as f32 * g);
            let zona = zona_en(x, y) as f32;
            // Ángulo base de las líneas con ondulación suave; su tensor tiene el
            // «gradiente» perpendicular a ese ángulo.
            let a = base_ang + 0.45 * ruido.angulo(x, y).sin() + zona * 0.6;
            let (na, nb) = (-(a.sin()), a.cos());
            let peso_base = 0.02 + 0.5 * (1.0 - op.sigue).powi(2);
            let (mut txx, mut txy, mut tyy) = (peso_base * na * na, peso_base * na * nb, peso_base * nb * nb);
            for (p, (a, b, c)) in &escalas {
                txx += op.sigue.max(0.05) * p * a.v[k];
                txy += op.sigue.max(0.05) * p * b.v[k];
                tyy += op.sigue.max(0.05) * p * c.v[k];
            }
            // Dirección del gradiente dominante; las líneas van a 90° de él.
            let a_grad = 0.5 * (2.0 * txy).atan2(txx - tyy);
            let a_linea = a_grad + std::f32::consts::FRAC_PI_2 + amplitud * ondas.angulo(x, y).sin();
            cx[k] = (2.0 * a_linea).cos();
            cy[k] = (2.0 * a_linea).sin();
        }
    }
    let mut dir_x = Rejilla { w, h, paso: g, v: cx };
    let mut dir_y = Rejilla { w, h, paso: g, v: cy };
    // Direcciones suaves: curvas sin quiebros.
    dir_x.suavizar(2);
    dir_y.suavizar(2);
    let direccion = |x: f32, y: f32| -> (f32, f32) {
        let a = 0.5 * dir_y.muestra(x, y).atan2(dir_x.muestra(x, y));
        (a.cos(), a.sin())
    };

    // ---- Jobard–Lefer
    let paso_l = 0.1 * periodo;
    // Una línea se detiene al acercarse a otra a esta fracción de la separación: baja = líneas largas (espagueti).
    let prueba: f32 = std::env::var("DEDALO_PRUEBA").ok().and_then(|v| v.parse().ok()).unwrap_or(0.35);
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
        if !dentro(sx, sy) || en_fondo(sx, sy) || espacio.ocupado(sx, sy, 0.9 * sep(sx, sy), u32::MAX, 0, 0) {
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
                if !dentro(nx, ny) || zona_en(nx, ny) != zona || en_fondo(nx, ny) {
                    break;
                }
                let idx = if lado == 0 { n as i32 } else { -(n as i32) };
                if espacio.ocupado(nx, ny, (prueba * sep(nx, ny)).max(cabe), id, idx, salto) {
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
        // Trazos cortos también: mejor un guion que un hueco.
        if largo < 0.5 * sep(sx, sy) {
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
        suavizar_linea(&mut linea, 12);
        lineas.push(linea);
    }

    // ---- Campo: distancia a la línea más cercana / separación local.
    // Rejilla fina (independiente de la separación): las líneas más finas (muro
    // mínimo) deben medir varias muestras.
    let paso = (0.55 * periodo / 12.0).min(op.muro_min / 4.0).max(0.04);
    let salto_campo = (2.0 * s_max / paso_l).ceil() as i32;
    let mut seg = Espacio::new(ancho, alto, s_max);
    for (id, l) in lineas.iter().enumerate() {
        for (k, &(x, y)) in l.iter().enumerate() {
            seg.poner(Punto { x, y, linea: id as u32, idx: k as i32 });
        }
    }
    let (cw, ch) = ((ancho / paso).ceil() as usize + 1, (alto / paso).ceil() as usize + 1);
    let mut v = vec![s_max; cw * ch];
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
            // Grosor en mm: el tono es la fracción del hueco entre las dos líneas
            // que lleva tinta, sin bajar del muro mínimo ni dejar un canal menor
            // que el mínimo. El campo es una distancia (mm): bordes lisos.
            let hueco = d1 + d2;
            // Tono fino (no la rejilla gruesa): el grosor cambia a lo largo de la
            // línea con el detalle de la imagen, como en el grabado.
            let t = tono_en(x, y);
            let medio = (t * hueco / 2.0).max(op.muro_min / 2.0);
            let tope = ((hueco - op.canal_min) / 2.0).max(op.muro_min / 2.0);
            *o = d1 - medio.min(tope);
        }
        let n = hechas.fetch_add(1, Ordering::Relaxed);
        ctl.avance.store((800 + n * 200 / ch) as u32, Ordering::Relaxed);
    });
    // Suavizado [1 2 1] (una pasada): redondea los quiebros donde el grosor
    // permitido cambia de golpe (p. ej. donde empieza o acaba una línea vecina).
    for _ in 0..1 {
        let t = v.clone();
        for j in 0..ch {
            for i in 0..cw {
                let a = t[j * cw + i.saturating_sub(1)];
                let b = t[j * cw + (i + 1).min(cw - 1)];
                v[j * cw + i] = 0.25 * a + 0.5 * t[j * cw + i] + 0.25 * b;
            }
        }
        let t = v.clone();
        for j in 0..ch {
            for i in 0..cw {
                let a = t[j.saturating_sub(1) * cw + i];
                let b = t[(j + 1).min(ch - 1) * cw + i];
                v[j * cw + i] = 0.25 * a + 0.5 * t[j * cw + i] + 0.25 * b;
            }
        }
    }
    Some(Campo { w: cw, h: ch, v, paso, x0: 0.0, y0: 0.0, ancho, alto, solucion: vec![], recortar: true, fijo: true })
}

/// (radio de suavizado en celdas de medio periodo, peso) de cada escala del tensor.
/// Pruebas: DEDALO_ESCALAS="r:p,r:p,…".
fn escalas_tensor() -> Vec<(usize, f32)> {
    std::env::var("DEDALO_ESCALAS")
        .ok()
        .map(|v| v.split(',').filter_map(|e| e.split_once(':').and_then(|(r, p)| Some((r.parse().ok()?, p.parse().ok()?)))).collect())
        .unwrap_or_else(|| vec![(1, 1.0), (3, 0.8), (8, 0.6), (20, 0.5)])
}

/// Media móvil [¼ ½ ¼] (`pasadas` veces) sin mover los extremos.
fn suavizar_linea(l: &mut [(f32, f32)], pasadas: usize) {
    if l.len() < 3 {
        return;
    }
    let mut prev = l.to_vec();
    for _ in 0..pasadas {
        for k in 1..l.len() - 1 {
            l[k] = (0.25 * prev[k - 1].0 + 0.5 * prev[k].0 + 0.25 * prev[k + 1].0, 0.25 * prev[k - 1].1 + 0.5 * prev[k].1 + 0.25 * prev[k + 1].1);
        }
        prev.copy_from_slice(l);
    }
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
    fn mas_tinta_en_lo_oscuro() {
        // Mitad izquierda blanca, mitad derecha negra.
        let img = image::GrayImage::from_fn(200, 100, |x, _| image::Luma([if x < 100 { 255 } else { 0 }]));
        let tono = Tono::new(&img, 100.0, 50.0, 4.0);
        let ajuste = Ajuste { niveles: false, ..Default::default() };
        let c = flujo(100.0, 50.0, 4.0, 3, Some(&tono), ajuste, Opciones { zonas: 1, sigue: 0.0, canal_min: 1.0, muro_min: 0.4, fondo_vacio: false }, &Control::default()).unwrap();
        // Fracción con tinta (campo < 0) a cada lado, lejos del borde central.
        let tinta = |x0: f32, x1: f32| {
            let (mut n, mut t) = (0, 0);
            for j in 0..c.h {
                for i in 0..c.w {
                    let (x, y) = c.pos(i, j);
                    if x > x0 && x < x1 && y > 3.0 && y < 47.0 {
                        n += 1;
                        t += (c.at(i, j) < 0.0) as usize;
                    }
                }
            }
            t as f32 / n as f32
        };
        let (claro, oscuro) = (tinta(5.0, 40.0), tinta(60.0, 95.0));
        assert!(claro > 0.03 && claro < 0.25, "claro {claro}");
        assert!(oscuro > 0.5, "oscuro {oscuro}");
    }
}

#[cfg(test)]
mod diagnostico {
    use super::*;
    use crate::contorno::{Dibujo, Limites};
    use crate::patron::Umbrales;

    /// Cuántos píxeles de la vista previa quedan en gris intermedio (bordes suavizados).
    #[test]
    #[ignore]
    fn bordes_suavizados() {
        let img = image::GrayImage::from_fn(200, 200, |x, y| image::Luma([((x + y) / 2) as u8]));
        let tono = Tono::new(&img, 60.0, 60.0, 4.0);
        let op = Opciones { zonas: 1, sigue: 0.0, canal_min: 1.2, muro_min: 0.4, fondo_vacio: false };
        let c = flujo(60.0, 60.0, 4.0, 1, Some(&tono), Ajuste::default(), op, &Control::default()).unwrap();
        let u = Umbrales::new(&c);
        let d = Dibujo { campo: &c, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(4.0, 0.4, 1.2) };
        let r = d.raster_region((20.0, 20.0, 40.0, 40.0), 400, 400, None);
        let grises = r.chunks(4).filter(|p| p[0] > 10 && p[0] < 245).count();
        println!("intermedios: {} de {}", grises, 400 * 400);
        // Pendiente típica.
        let mut ds = vec![];
        for k in 0..20 {
            let (x, y) = (25.0 + k as f32 * 0.37, 30.0);
            let g = d.valor(x, y);
            let gx = d.valor(x + 0.05, y) - g;
            let gy = d.valor(x, y + 0.05) - g;
            ds.push(format!("{g:.3}/{:.4}", (gx * gx + gy * gy).sqrt()));
        }
        println!("{}", ds.join(" "));
    }
}

#[cfg(test)]
mod retratos {
    use super::*;
    use crate::contorno::{Dibujo, Limites};
    use crate::patron::Umbrales;

    /// DEDALO_RETRATOS=carpeta DEDALO_SALIDA=carpeta: dibuja cada imagen con las
    /// líneas de flujo (ajustes por defecto, cuadro de 15 cm) junto al original.
    /// DEDALO_SIGUE=0.8 DEDALO_ZONAS=1 cambian esos ajustes.
    #[test]
    #[ignore]
    fn hoja_de_retratos() {
        let (Ok(dir), Ok(sal)) = (std::env::var("DEDALO_RETRATOS"), std::env::var("DEDALO_SALIDA")) else { return };
        let sigue = std::env::var("DEDALO_SIGUE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.5);
        let zonas = std::env::var("DEDALO_ZONAS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let periodo = std::env::var("DEDALO_PERIODO").ok().and_then(|v| v.parse().ok()).unwrap_or(4.0);
        let mut nombres: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "jpg" || x == "png")).collect();
        nombres.sort();
        for p in nombres {
            let img = crate::tono::abrir(&p).unwrap();
            let asp = img.width() as f32 / img.height() as f32;
            let lado: f32 = std::env::var("DEDALO_LADO").ok().and_then(|v| v.parse().ok()).unwrap_or(150.0);
            let (ancho, alto) = if asp > 1.0 { (lado, lado / asp) } else { (lado * asp, lado) };
            let tono = Tono::new(&img, ancho, alto, periodo);
            let op = Opciones { zonas, sigue, canal_min: 1.2, muro_min: 0.4, fondo_vacio: false };
            let t = std::time::Instant::now();
            let c = flujo(ancho, alto, periodo, 1, Some(&tono), Ajuste::default(), op, &Control::default()).unwrap();
            let u = Umbrales::new(&c);
            let d = Dibujo { campo: &c, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(periodo, 0.4, 1.2) };
            let py = 900usize;
            let px = (py as f32 * ancho / alto) as usize;
            let r = d.raster(px, py, None);
            let dib = image::RgbaImage::from_raw(px as u32, py as u32, r).unwrap();
            let orig = image::imageops::resize(&image::open(&p).unwrap().to_rgba8(), px as u32, py as u32, image::imageops::FilterType::Triangle);
            let mut hoja = image::RgbaImage::from_pixel(px as u32 * 2 + 20, py as u32, image::Rgba([255, 255, 255, 255]));
            image::imageops::overlay(&mut hoja, &orig, 0, 0);
            image::imageops::overlay(&mut hoja, &dib, px as i64 + 20, 0);
            let nombre = p.file_stem().unwrap().to_string_lossy().into_owned();
            hoja.save(format!("{sal}/{nombre}.png")).unwrap();
            println!("{nombre}: {:?}", t.elapsed());
        }
    }
}
