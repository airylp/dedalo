//! Patrones: un campo continuo sobre el área de dibujo donde los valores bajos
//! son «muro» (tinta). El tono de la imagen decide luego, punto a punto, hasta
//! qué valor se entinta (`Umbrales`), así los muros engordan en las zonas
//! oscuras y adelgazan en las claras sin romper la forma del patrón.
//!
//! - Reacción-difusión (Gray-Scott, ajuste «laberinto»: F = 0.029, k = 0.057):
//!   rayas que se ramifican, como una huella dactilar.
//! - Laberinto perfecto (búsqueda en profundidad): un solo camino entre la
//!   entrada (arriba a la izquierda) y la salida (abajo a la derecha). El campo
//!   es la distancia al muro más cercano.

use rayon::prelude::*;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Tipo {
    /// Líneas de flujo (ver `flujo.rs`).
    #[serde(rename = "flujo")]
    Flujo,
    #[serde(rename = "reaccion")]
    Reaccion,
    #[serde(rename = "laberinto")]
    Laberinto,
    /// Una línea en espiral desde el centro (ver `espiral.rs`).
    #[serde(rename = "espiral")]
    Espiral,
    /// Una línea en espiral que se aprieta en lo oscuro y se abre en lo claro (`espiral::espiral_densa`).
    #[serde(rename = "espiral_densa")]
    EspiralDensa,
}

/// Valores muestreados en una rejilla regular (en mm, desde la esquina superior
/// izquierda del área de dibujo; puede empezar antes de 0 para incluir medio muro).
#[derive(Clone)]
pub struct Campo {
    pub w: usize,
    pub h: usize,
    pub v: Vec<f32>,
    /// Distancia entre muestras (mm).
    pub paso: f32,
    /// Posición (mm) de la muestra (0, 0).
    pub x0: f32,
    pub y0: f32,
    /// Lo que mide el área de dibujo (mm).
    pub ancho: f32,
    pub alto: f32,
    /// Solo laberinto: camino de la entrada a la salida (centros de celda, mm).
    pub solucion: Vec<(f32, f32)>,
    /// Lo que queda fuera del área se corta (reacción-difusión: es el margen de
    /// la simulación). El laberinto conserva entero su muro exterior.
    pub recortar: bool,
    /// El campo ya trae el tono aplicado (negativo = tinta): el umbral es 0.
    pub fijo: bool,
}

impl Campo {
    pub fn at(&self, i: usize, j: usize) -> f32 {
        self.v[j * self.w + i]
    }

    /// Valor interpolado en (x, y) mm.
    pub fn muestra(&self, x: f32, y: f32) -> f32 {
        let fx = ((x - self.x0) / self.paso).clamp(0.0, (self.w - 1) as f32);
        let fy = ((y - self.y0) / self.paso).clamp(0.0, (self.h - 1) as f32);
        let (i, j) = (fx.floor() as usize, fy.floor() as usize);
        let (i1, j1) = ((i + 1).min(self.w - 1), (j + 1).min(self.h - 1));
        let (tx, ty) = (fx - i as f32, fy - j as f32);
        let a = self.at(i, j) * (1.0 - tx) + self.at(i1, j) * tx;
        let b = self.at(i, j1) * (1.0 - tx) + self.at(i1, j1) * tx;
        a * (1.0 - ty) + b * ty
    }

    /// Posición (mm) de la muestra (i, j).
    pub fn pos(&self, i: usize, j: usize) -> (f32, f32) {
        (self.x0 + i as f32 * self.paso, self.y0 + j as f32 * self.paso)
    }
}

/// Generador pseudoaleatorio pequeño y reproducible (misma semilla = mismo patrón).
pub struct Azar(u64);

impl Azar {
    pub fn new(semilla: u64) -> Azar {
        Azar(semilla.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03)
    }
    pub fn next(&mut self) -> u64 {
        // splitmix64
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    pub fn real(&mut self) -> f32 {
        (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
    pub fn hasta(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// Avance (0–1000) y cancelación de un cálculo largo.
#[derive(Default)]
pub struct Control {
    pub avance: AtomicU32,
    pub cancelar: AtomicBool,
}

impl Control {
    fn cancelado(&self) -> bool {
        self.cancelar.load(Ordering::Relaxed)
    }
    fn avanza(&self, hecho: usize, total: usize) {
        self.avance.store((hecho * 1000 / total.max(1)) as u32, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------- reacción-difusión

/// Muestras por periodo de las rayas con F = 0.029, k = 0.057, Du = 1, Dv = 0.5
/// (medido: test `periodo_de_las_rayas`).
pub const RD_MUESTRAS_POR_PERIODO: f32 = 16.0;
const RD_F: f32 = 0.029;
const RD_K: f32 = 0.057;
const RD_DU: f32 = 1.0;
const RD_DV: f32 = 0.5;
/// Pasos de la simulación como máximo (antes se daban siempre todos).
pub const RD_PASOS: usize = 7000;
/// Las rayas nunca se quedan quietas: tras formarse (≈ 2000–4000 pasos) siguen
/// moviéndose despacio a ritmo fijo. Se para cuando el cambio medio de V en
/// `RD_CADA` pasos baja de esto (≈ 4000–4500 pasos; medido con `traza_rd`).
const RD_QUIETO: f32 = 0.0045;
const RD_CADA: usize = 250;
// (no antes de 2000 pasos: al principio las manchas crecen y el cambio puede ser bajo).
/// Pasos que suele dar (para la barra de avance).
const RD_PASOS_TIPICOS: usize = 4500;

/// Rayas de reacción-difusión sobre un área de `ancho` × `alto` mm cuyo
/// periodo (centro de un canal al centro del siguiente) es `periodo` mm.
pub fn reaccion_difusion(ancho: f32, alto: f32, periodo: f32, semilla: u64, pasos: usize, ctl: &Control) -> Option<Campo> {
    let paso = periodo / RD_MUESTRAS_POR_PERIODO;
    // Un periodo de margen alrededor: el borde de la simulación se ve distinto.
    let x0 = -periodo;
    let y0 = -periodo;
    let w = ((ancho + 2.0 * periodo) / paso).ceil() as usize + 1;
    let h = ((alto + 2.0 * periodo) / paso).ceil() as usize + 1;
    let n = w * h;
    let mut u = vec![1.0f32; n];
    let mut v = vec![0.0f32; n];
    let mut az = Azar::new(semilla);
    // Semillas: manchitas repartidas por toda el área, para que el patrón
    // crezca a la vez en todas partes (y no desde el centro).
    let manchas = (n / 40).max(1);
    for _ in 0..manchas {
        let (ci, cj) = (az.hasta(w), az.hasta(h));
        for dj in 0..2 {
            for di in 0..2 {
                let (i, j) = ((ci + di).min(w - 1), (cj + dj).min(h - 1));
                u[j * w + i] = 0.5;
                v[j * w + i] = 0.25 + 0.05 * az.real();
            }
        }
    }
    let mut u2 = vec![0.0f32; n];
    let mut v_antes = v.clone();
    let mut v2 = vec![0.0f32; n];
    for it in 0..pasos {
        if it % 50 == 0 {
            if ctl.cancelado() {
                return None;
            }
            ctl.avanza(it.min(RD_PASOS_TIPICOS * 19 / 20), pasos.min(RD_PASOS_TIPICOS));
        }
        u2.par_chunks_mut(w).zip(v2.par_chunks_mut(w)).enumerate().for_each(|(j, (fu, fv))| {
            let jm = if j == 0 { 1 } else { j - 1 };
            let jp = if j + 1 == h { h - 2 } else { j + 1 };
            let (um, uc, up) = (&u[jm * w..][..w], &u[j * w..][..w], &u[jp * w..][..w]);
            let (vm, vc, vp) = (&v[jm * w..][..w], &v[j * w..][..w], &v[jp * w..][..w]);
            rd_fila([um, uc, up], [vm, vc, vp], fu, fv);
        });
        std::mem::swap(&mut u, &mut u2);
        std::mem::swap(&mut v, &mut v2);
        if (it + 1) % RD_CADA == 0 {
            let cambio = v.iter().zip(&v_antes).map(|(a, b)| (a - b).abs()).sum::<f32>() / n as f32;
            if cambio < RD_QUIETO && it + 1 >= 2000 {
                break;
            }
            v_antes.copy_from_slice(&v);
        }
    }
    ctl.avanza(1, 1);
    // V alto = raya: se invierte para que «bajo = muro».
    let campo: Vec<f32> = v.iter().map(|x| -x).collect();
    Some(Campo { w, h, v: campo, paso, x0, y0, ancho, alto, solucion: vec![], recortar: true, fijo: false })
}

/// Un paso de Gray-Scott en una fila (`u`, `v` = fila de arriba, la misma y la de abajo;
/// bordes reflejados). Cortes de largo fijo: sin comprobaciones de límites, se vectoriza.
fn rd_fila(u: [&[f32]; 3], v: [&[f32]; 3], fu: &mut [f32], fv: &mut [f32]) {
    let w = fu.len();
    #[inline(always)]
    fn celda(u: [&[f32]; 3], v: [&[f32]; 3], im: usize, i: usize, ip: usize) -> (f32, f32) {
        let lap = |[m, c, p]: [&[f32]; 3]| 0.2 * (c[im] + c[ip] + m[i] + p[i]) + 0.05 * (m[im] + m[ip] + p[im] + p[ip]) - c[i];
        let (uu, vv) = (u[1][i], v[1][i]);
        let uvv = uu * vv * vv;
        ((uu + RD_DU * lap(u) - uvv + RD_F * (1.0 - uu)).clamp(0.0, 1.0), (vv + RD_DV * lap(v) + uvv - (RD_F + RD_K) * vv).clamp(0.0, 1.0))
    }
    (fu[0], fv[0]) = celda(u, v, 1, 0, 1);
    (fu[w - 1], fv[w - 1]) = celda(u, v, w - 2, w - 1, w - 2);
    let n = w - 2;
    let [um, uc, up] = u.map(|f| &f[..n + 2]);
    let [vm, vc, vp] = v.map(|f| &f[..n + 2]);
    let (fu, fv) = (&mut fu[1..n + 1], &mut fv[1..n + 1]);
    for k in 0..n {
        let lap = |m: &[f32], c: &[f32], p: &[f32]| 0.2 * (c[k] + c[k + 2] + m[k + 1] + p[k + 1]) + 0.05 * (m[k] + m[k + 2] + p[k] + p[k + 2]) - c[k + 1];
        let (uu, vv) = (uc[k + 1], vc[k + 1]);
        let uvv = uu * vv * vv;
        fu[k] = (uu + RD_DU * lap(um, uc, up) - uvv + RD_F * (1.0 - uu)).clamp(0.0, 1.0);
        fv[k] = (vv + RD_DV * lap(vm, vc, vp) + uvv - (RD_F + RD_K) * vv).clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------- laberinto

/// Muestras por celda del laberinto.
const LAB_MUESTRAS: usize = 12;

/// Laberinto perfecto de celdas de `periodo` mm (muro + canal). Las celdas se
/// ajustan para llenar el ancho exacto; el alto se redondea a celdas enteras.
pub fn laberinto(ancho: f32, alto: f32, periodo: f32, semilla: u64, ctl: &Control) -> Option<Campo> {
    let cols = ((ancho / periodo).round() as usize).max(2);
    let celda = ancho / cols as f32;
    let filas = ((alto / celda).round() as usize).max(2);
    // Muros: verticales vw[j][i] (i en 0..=cols) y horizontales hw[j][i] (j en 0..=filas).
    let mut vw = vec![true; filas * (cols + 1)];
    let mut hw = vec![true; (filas + 1) * cols];
    let mut visto = vec![false; cols * filas];
    let mut az = Azar::new(semilla);
    let mut pila = vec![(0usize, 0usize)];
    visto[0] = true;
    while let Some(&(i, j)) = pila.last() {
        let mut vecinos = [(0usize, 0usize, 0u8); 4];
        let mut k = 0;
        if i > 0 && !visto[j * cols + i - 1] {
            vecinos[k] = (i - 1, j, 0);
            k += 1;
        }
        if i + 1 < cols && !visto[j * cols + i + 1] {
            vecinos[k] = (i + 1, j, 1);
            k += 1;
        }
        if j > 0 && !visto[(j - 1) * cols + i] {
            vecinos[k] = (i, j - 1, 2);
            k += 1;
        }
        if j + 1 < filas && !visto[(j + 1) * cols + i] {
            vecinos[k] = (i, j + 1, 3);
            k += 1;
        }
        if k == 0 {
            pila.pop();
            continue;
        }
        let (ni, nj, dir) = vecinos[az.hasta(k)];
        match dir {
            0 => vw[j * (cols + 1) + i] = false,
            1 => vw[j * (cols + 1) + i + 1] = false,
            2 => hw[j * cols + i] = false,
            _ => hw[(j + 1) * cols + i] = false,
        }
        visto[nj * cols + ni] = true;
        pila.push((ni, nj));
    }
    // Entrada y salida.
    hw[0] = false;
    hw[filas * cols + cols - 1] = false;
    if ctl.cancelado() {
        return None;
    }
    let solucion = resolver(cols, filas, &vw, &hw).into_iter().map(|(i, j)| ((i as f32 + 0.5) * celda, (j as f32 + 0.5) * celda)).collect();

    // Distancia (en celdas) al muro más cercano, con medio periodo de margen.
    let paso_c = 1.0 / LAB_MUESTRAS as f32;
    let (x0c, y0c) = (-0.5f32, -0.5f32);
    let w = (cols + 1) * LAB_MUESTRAS + 1;
    let h = (filas + 1) * LAB_MUESTRAS + 1;
    let seg_v = |i: isize, j: isize| -> bool { i >= 0 && j >= 0 && (i as usize) <= cols && (j as usize) < filas && vw[j as usize * (cols + 1) + i as usize] };
    let seg_h = |i: isize, j: isize| -> bool { i >= 0 && j >= 0 && (i as usize) < cols && (j as usize) <= filas && hw[j as usize * cols + i as usize] };
    let mut v = vec![0.0f32; w * h];
    v.par_chunks_mut(w).enumerate().for_each(|(jj, fila)| {
        let y = y0c + jj as f32 * paso_c;
        for (ii, out) in fila.iter_mut().enumerate() {
            let x = x0c + ii as f32 * paso_c;
            let (ci, cj) = (x.floor() as isize, y.floor() as isize);
            let mut d = f32::MAX;
            for dj in -1..=1 {
                for di in -1..=1 {
                    let (a, b) = (ci + di, cj + dj);
                    // Muro vertical en x = a, de y = b a b + 1.
                    if seg_v(a, b) {
                        d = d.min(dist_seg(x, y, a as f32, b as f32, a as f32, b as f32 + 1.0));
                    }
                    // Muro horizontal en y = b, de x = a a a + 1.
                    if seg_h(a, b) {
                        d = d.min(dist_seg(x, y, a as f32, b as f32, a as f32 + 1.0, b as f32));
                    }
                }
            }
            *out = d.min(1.0);
        }
    });
    Some(Campo { w, h, v, paso: paso_c * celda, x0: x0c * celda, y0: y0c * celda, ancho, alto: filas as f32 * celda, solucion, recortar: false, fijo: false })
}

fn dist_seg(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let t = (((px - ax) * dx + (py - ay) * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
    let (qx, qy) = (ax + t * dx - px, ay + t * dy - py);
    (qx * qx + qy * qy).sqrt()
}

/// Camino de (0,0) a (cols-1, filas-1) (búsqueda en anchura).
fn resolver(cols: usize, filas: usize, vw: &[bool], hw: &[bool]) -> Vec<(usize, usize)> {
    let n = cols * filas;
    let mut previo = vec![usize::MAX; n];
    let mut cola = std::collections::VecDeque::from([0usize]);
    previo[0] = 0;
    while let Some(c) = cola.pop_front() {
        let (i, j) = (c % cols, c / cols);
        let mut ir = |d: usize| {
            if previo[d] == usize::MAX {
                previo[d] = c;
                cola.push_back(d);
            }
        };
        if i > 0 && !vw[j * (cols + 1) + i] {
            ir(c - 1);
        }
        if i + 1 < cols && !vw[j * (cols + 1) + i + 1] {
            ir(c + 1);
        }
        if j > 0 && !hw[j * cols + i] {
            ir(c - cols);
        }
        if j + 1 < filas && !hw[(j + 1) * cols + i] {
            ir(c + cols);
        }
    }
    let mut camino = vec![];
    let mut c = n - 1;
    if previo[c] == usize::MAX {
        return camino;
    }
    loop {
        camino.push((c % cols, c / cols));
        if c == 0 {
            break;
        }
        c = previo[c];
    }
    camino.reverse();
    camino
}

// ---------------------------------------------------------------- umbrales

/// Del tono (0 = blanco, 1 = negro) al valor del campo hasta el que se entinta:
/// el cuantil del campo, así en una zona de tono `t` queda entintada una
/// fracción `t` de la superficie (como una trama de imprenta).
#[derive(Clone)]
pub struct Umbrales {
    orden: Vec<f32>,
}

impl Umbrales {
    pub fn new(c: &Campo) -> Umbrales {
        // Puntos al azar dentro del área de dibujo (interpolados: en la rejilla
        // el laberinto da valores repetidos y los cuantiles saldrían a saltos).
        if c.fijo {
            return Umbrales { orden: vec![0.0] };
        }
        let mut az = Azar::new(0x5eed);
        let mut orden: Vec<f32> = (0..200_000).map(|_| c.muestra(az.real() * c.ancho, az.real() * c.alto)).collect();
        orden.sort_by(f32::total_cmp);
        Umbrales { orden }
    }

    pub fn de(&self, tono: f32) -> f32 {
        let n = self.orden.len();
        let f = tono.clamp(0.0, 1.0) * (n - 1) as f32;
        let i = f.floor() as usize;
        let j = (i + 1).min(n - 1);
        self.orden[i] + (self.orden[j] - self.orden[i]) * (f - i as f32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mide cuántas muestras ocupa un periodo de las rayas (cruces por la media
    /// en filas y columnas; en un patrón isótropo, periodo = 4/π · largo / cruces).
    #[test]
    #[ignore]
    fn periodo_de_las_rayas() {
        let ctl = Control::default();
        let c = reaccion_difusion(400.0, 400.0, RD_MUESTRAS_POR_PERIODO * 2.0, 3, RD_PASOS, &ctl).unwrap();
        // Con paso = 2 mm: 200 × 200 muestras útiles.
        let media = c.v.iter().sum::<f32>() / c.v.len() as f32;
        let (mut cruces, mut largo) = (0usize, 0usize);
        for j in (20..c.h - 20).step_by(7) {
            for i in 21..c.w - 20 {
                if (c.at(i - 1, j) < media) != (c.at(i, j) < media) {
                    cruces += 1;
                }
                largo += 1;
            }
        }
        let p = 4.0 / std::f32::consts::PI * largo as f32 / cruces as f32;
        println!("periodo medido: {p:.2} muestras");
    }

    #[test]
    fn laberinto_tiene_solucion_y_muros() {
        let c = laberinto(100.0, 60.0, 5.0, 7, &Control::default()).unwrap();
        assert_eq!(c.ancho, 100.0);
        assert!(!c.solucion.is_empty());
        let primero = c.solucion[0];
        let ultimo = *c.solucion.last().unwrap();
        assert!(primero.0 < 5.0 && primero.1 < 5.0);
        assert!(ultimo.0 > 95.0 && ultimo.1 > c.alto - 5.0);
        // En el centro de una celda nunca hay muro.
        assert!(c.muestra(2.5, 2.5) > 0.3);
        // La pared izquierda está entera (salvo nada: la entrada va arriba).
        assert!(c.muestra(0.0, 30.0) < 0.01);
    }

    #[test]
    fn umbrales_reparten_por_cuantiles() {
        let c = laberinto(80.0, 80.0, 5.0, 1, &Control::default()).unwrap();
        let u = Umbrales::new(&c);
        for t in [0.2f32, 0.5, 0.8] {
            let th = u.de(t);
            let mut az = Azar::new(99);
            let dentro = 50_000;
            let tinta = (0..dentro).filter(|_| c.muestra(az.real() * c.ancho, az.real() * c.alto) < th).count();
            let f = tinta as f32 / dentro as f32;
            assert!((f - t).abs() < 0.05, "tono {t}: entintado {f}");
        }
    }

    #[test]
    fn misma_semilla_mismo_patron() {
        let ctl = Control::default();
        let a = reaccion_difusion(20.0, 20.0, 4.0, 9, 300, &ctl).unwrap();
        let b = reaccion_difusion(20.0, 20.0, 4.0, 9, 300, &ctl).unwrap();
        assert_eq!(a.v, b.v);
    }
}

#[cfg(test)]
mod tiempo {
    use super::*;
    /// Cuánto tarda la reacción-difusión en una hoja carta (separación 4 mm).
    #[test]
    #[ignore]
    fn tiempo_hoja_carta() {
        let t = std::time::Instant::now();
        let c = reaccion_difusion(186.0, 232.0, 4.0, 1, RD_PASOS, &Control::default()).unwrap();
        println!("{}×{} muestras: {:?}", c.w, c.h, t.elapsed());
    }
}
