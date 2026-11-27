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
use crate::rostro::{Cara, cerca};
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
    /// Grosor máximo de una línea (mm; `f32::MAX` = sin límite).
    pub grosor_max: f32,
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

/// Segmento de una línea (de su punto `idx` al siguiente) y la celda de `Espacio` donde empieza.
#[derive(Clone, Copy)]
struct Segmento {
    a: (f32, f32),
    b: (f32, f32),
    linea: u32,
    idx: i32,
    celda: (usize, usize),
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
pub fn flujo(ancho: f32, alto: f32, periodo: f32, semilla: u64, tono: Option<&Tono>, ajuste: Ajuste, op: Opciones, cara: Option<&Cara>, ctl: &Control) -> Option<Campo> {
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
    let g = periodo / 2.0;
    let mut tonos = Rejilla::new(ancho, alto, g, tono_en);
    tonos.suavizar(1);

    // Retrato: más detalle (líneas más juntas) junto a ojos, cejas, nariz y labios;
    // regiones cara / pelo / resto (las líneas no pasan de una a otra).
    let detalle = Rejilla::new(ancho, alto, g, |x, y| {
        let Some(c) = cara else { return 0.0 };
        let d = c.rasgos.iter().map(|l| cerca(l, x, y).0).fold(f32::MAX, f32::min);
        let ojo = c.ojos.iter().map(|o| 1.0 - suave_paso(1.3, 2.0, o.polar(x, y).0)).fold(0.0, f32::max);
        (1.0 - suave_paso(0.6 * periodo, 2.2 * periodo, d)).max(ojo)
    });
    let region = match cara {
        Some(c) => {
            // Pelo: alrededor de la cabeza y de un tono distinto al del fondo (mediana de la franja de arriba).
            let mut arriba: Vec<f32> = (0..tonos.w).flat_map(|i| (0..(tonos.h / 12).max(1)).map(move |j| (i, j))).map(|(i, j)| tonos.at(i, j)).collect();
            arriba.sort_by(f32::total_cmp);
            let fondo_t = arriba[arriba.len() / 2];
            let mut pelo = Rejilla::new(ancho, alto, g, |x, y| (c.zona_pelo(x, y) && (tonos.muestra(x, y) - fondo_t).abs() > 0.12) as u8 as f32);
            pelo.suavizar(2);
            pelo.suavizar(2);
            Rejilla::new(ancho, alto, g, |x, y| if c.dentro(x, y) { 1.0 } else if pelo.muestra(x, y) > 0.5 { 2.0 } else { 0.0 })
        }
        None => Rejilla { w: 1, h: 1, paso: g, v: vec![0.0] },
    };
    let region_en = |x: f32, y: f32| region.cercano(x, y) as u32;
    let k_detalle = if cara.is_some() { 0.7 } else { 1.0 };
    // Rostro: unas `LINEAS_CARA` líneas a lo ancho de la cara (y algo menos en el pelo), sea cual
    // sea la separación general: con 4 mm en una cara de 6 cm solo cabían 12 y los rasgos se perdían.
    let lineas_cara: f32 = std::env::var("DEDALO_LINEAS_CARA").ok().and_then(|v| v.parse().ok()).unwrap_or(34.0);
    let k_cara = cara.map_or(1.0, |c| (2.0 * c.semi.0 / lineas_cara / periodo).clamp(0.4, 1.0));
    let mut denso = Rejilla::new(ancho, alto, g, |x, y| match region.cercano(x, y) as u32 {
        1 => k_cara,
        2 => 0.5 + 0.5 * k_cara,
        _ => 1.0,
    });
    denso.suavizar(2);
    denso.suavizar(2);
    let (s_max, s_min) = (k_claro.max(k_oscuro) * periodo, k_claro.min(k_oscuro) * periodo * k_detalle * k_cara);
    let sep = |x: f32, y: f32| (k_claro * periodo + (k_oscuro - k_claro) * periodo * tonos.muestra(x, y)) * (1.0 - (1.0 - k_detalle) * detalle.muestra(x, y) * (1.0 - tonos.muestra(x, y))) * denso.muestra(x, y);
    // (solo en lo claro: en lo oscuro, juntar las líneas deja sin sitio la tinta por el canal mínimo).

    // Zonas: bandas de tono muy suavizado (y, en un retrato, cara / pelo / resto).
    let zonas_n = op.zonas.clamp(1, 8);
    let mut suave = Rejilla { w: tonos.w, h: tonos.h, paso: g, v: tonos.v.clone() };
    suave.suavizar(4);
    suave.suavizar(4);
    let zona_en = |x: f32, y: f32| -> u32 { ((suave.cercano(x, y) * zonas_n as f32) as u32).min(zonas_n - 1) * 3 + (region_en(x, y) == 2) as u32 };
    // (la cara no es zona aparte: el óvalo ya guía las líneas y cortarlas ahí dejaba trocitos sueltos).

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
    let env = |n: &str, d: f32| std::env::var(n).ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
    let (k_fino, k_ovalo, k_anillo) = (env("DEDALO_FINO", 3.0), env("DEDALO_OVALO", 1.2), env("DEDALO_ANILLO", 0.0));
    // Ondulación de espagueti: la dirección gira un poco con un ruido de escala media.
    let ondas = Ruido::new(semilla ^ 0x5A, 7.0 * periodo, ancho, alto);
    let amplitud = 0.35 * (1.0 - 0.5 * op.sigue);
    for j in 0..h {
        for i in 0..w {
            let k = j * w + i;
            let (x, y) = (i as f32 * g, j as f32 * g);
            let zona = (zona_en(x, y) / 3) as f32;
            let reg = region_en(x, y);
            // Ángulo base de las líneas con ondulación suave; su tensor tiene el
            // «gradiente» perpendicular a ese ángulo.
            let a = base_ang + 0.45 * ruido.angulo(x, y).sin() + zona * 0.6;
            let (na, nb) = (-(a.sin()), a.cos());
            let peso_base = 0.02 + 0.5 * (1.0 - op.sigue).powi(2);
            let mut t = [peso_base * na * na, peso_base * na * nb, peso_base * nb * nb];
            let mut amp = amplitud;
            if reg == 2 {
                // Pelo: mechones = solo las escalas finas, y una tendencia a salir de la cabeza.
                for (p, (a, b, c)) in escalas.iter().take(2) {
                    let n = (a.v[k], b.v[k], c.v[k]);
                    t[0] += 1.2 * p * n.0;
                    t[1] += 1.2 * p * n.1;
                    t[2] += 1.2 * p * n.2;
                }
                if let Some(c) = cara {
                    let (dx, dy) = (x - c.centro.0, y - c.centro.1);
                    let l = dx.hypot(dy).max(1e-6);
                    sumar(&mut t, 0.15, (-dy / l, dx / l));
                }
                amp = 0.0;
            } else {
                // En la cara cuentan más las escalas finas (arrugas, sombras de la expresión).
                for (e, (p, (a, b, c))) in escalas.iter().enumerate() {
                    let f = op.sigue.max(0.05) * p * if reg == 1 && e < 2 { k_fino } else { 1.0 };
                    t[0] += f * a.v[k];
                    t[1] += f * b.v[k];
                    t[2] += f * c.v[k];
                }
            }
            if let Some(c) = cara {
                // Las líneas corren paralelas al óvalo, a las cejas, la nariz y los labios…
                // (por dentro el óvalo alcanza poco: si no, la cara entera queda en anillos concéntricos).
                let (d, tg) = cerca(&c.ovalo, x, y);
                let alcance = if reg == 1 { k_ovalo * periodo * k_cara } else { 2.5 * periodo };
                sumar(&mut t, 1.5 * (-(d / alcance).powi(2)).exp(), (-tg.1, tg.0));
                for l in &c.guias {
                    let (d, tg) = cerca(l, x, y);
                    sumar(&mut t, 2.0 * (-(d / (1.2 * periodo * k_cara)).powi(2)).exp(), (-tg.1, tg.0));
                }
                // …y en el iris giran un poco.
                if k_anillo > 0.0 {
                    for o in &c.ojos {
                        let (rho, n) = o.polar(x, y);
                        sumar(&mut t, k_anillo * (1.0 - suave_paso(0.8, 1.3, rho)), n);
                    }
                }
                // La cara sin ondular; el fondo, tranquilo.
                amp *= if reg == 1 { 0.15 } else { 0.5 };
            }
            // Dirección del gradiente dominante; las líneas van a 90° de él.
            let [txx, txy, tyy] = t;
            let a_grad = 0.5 * (2.0 * txy).atan2(txx - tyy);
            let a_linea = a_grad + std::f32::consts::FRAC_PI_2 + amp * ondas.angulo(x, y).sin();
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
    let paso_l = 0.1 * periodo * k_cara;
    // Una línea se detiene al acercarse a otra a esta fracción de la separación: baja = líneas largas (espagueti).
    let prueba: f32 = std::env::var("DEDALO_PRUEBA").ok().and_then(|v| v.parse().ok()).unwrap_or(0.25);
    // Dos líneas nunca se acercan más de lo que cabe: un canal y dos medios muros… y algo de holgura.
    let cabe = 1.15 * (op.canal_min + op.muro_min);
    let mut espacio = Espacio::new(ancho, alto, s_max);
    let mut lineas: Vec<Vec<(f32, f32)>> = vec![];
    let mut zona_de: Vec<u32> = vec![];
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
    // Semillas a los lados de una línea, cada media separación a lo largo.
    let semillas_al_lado = |linea: &[(f32, f32)], cola: &mut VecDeque<(f32, f32)>| {
        let cada = ((0.5 * sep(linea[0].0, linea[0].1)) / paso_l).max(1.0) as usize;
        for k in (0..linea.len()).step_by(cada) {
            let (px, py) = linea[k];
            let (a, b) = (linea[k.saturating_sub(1)], linea[(k + 1).min(linea.len() - 1)]);
            let (tx, ty) = (b.0 - a.0, b.1 - a.1);
            let l = (tx * tx + ty * ty).sqrt().max(1e-6);
            let s = sep(px, py);
            cola.push_back((px - ty / l * s, py + tx / l * s));
            cola.push_back((px + ty / l * s, py - tx / l * s));
        }
    };
    // Retrato: primero los rasgos (cejas, párpados, boca, nariz) tal cual, para que una línea
    // pase justo por ellos; las demás se acomodan alrededor.
    let k_trazos = std::env::var("DEDALO_TRAZOS").map_or(true, |v| v != "0");
    for t in cara.filter(|_| k_trazos).map_or(&[][..], |c| &c.trazos[..]) {
        let id = lineas.len() as u32;
        let mut linea: Vec<(f32, f32)> = vec![];
        for p in curva_suave(t, paso_l) {
            if !dentro(p.0, p.1) || en_fondo(p.0, p.1) || espacio.ocupado(p.0, p.1, cabe, id, linea.len() as i32, i32::MAX) {
                break;
            }
            linea.push(p);
        }
        if (linea.len() as f32) * paso_l < sep(t[0].0, t[0].1) {
            continue;
        }
        for (k, &(x, y)) in linea.iter().enumerate() {
            espacio.poner(Punto { x, y, linea: id, idx: k as i32 });
        }
        semillas_al_lado(&linea, &mut cola);
        zona_de.push(zona_en(linea[linea.len() / 2].0, linea[linea.len() / 2].1));
        lineas.push(linea);
    }
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
        if largo < if cara.is_some() { 1.0 } else { 0.5 } * sep(sx, sy) {
            espacio.quitar_linea(id);
            continue;
        }
        let mut linea: Vec<(f32, f32)> = atras.into_iter().rev().collect();
        linea.push((sx, sy));
        linea.extend(adelante);
        semillas_al_lado(&linea, &mut cola);
        suavizar_linea(&mut linea, 12);
        lineas.push(linea);
        zona_de.push(zona);
    }

    // Líneas más continuas: se unen los extremos que se miran de frente y están cerca.
    let lineas = unir_extremos(lineas, &zona_de, &sep, paso_l);
    // ---- Campo: distancia a la línea más cercana / separación local.
    // Rejilla fina (independiente de la separación): las líneas más finas (muro
    // mínimo) deben medir varias muestras.
    let paso = (0.55 * periodo * k_cara / 12.0).min(op.muro_min / 4.0).max(0.04);
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
    // La línea más cercana (d1) y la más cercana de las demás (d2) desde (x, y),
    // mirando los segmentos de `segs` que están en las 3 × 3 celdas de su alrededor.
    let mas_cercanas = |x: f32, y: f32, segs: &mut dyn Iterator<Item = &Segmento>| -> (f32, f32) {
        let (ci, cj) = seg.celda(x, y);
        let (mut d1, mut id1, mut idx1) = (f32::MAX, u32::MAX, 0i32);
        let mut d2 = f32::MAX;
        for s in segs {
            if s.celda.0.abs_diff(ci) > 1 || s.celda.1.abs_diff(cj) > 1 {
                continue;
            }
            let d = dist2_seg(x, y, s.a.0, s.a.1, s.b.0, s.b.1);
            let misma = s.linea == id1 && (s.idx - idx1).abs() < salto_campo;
            if d < d1 {
                if !misma {
                    d2 = d2.min(d1);
                }
                (d1, id1, idx1) = (d, s.linea, s.idx);
            } else if !misma {
                d2 = d2.min(d);
            }
        }
        (d1, d2)
    };
    // Segmentos (punto → el siguiente de su línea) de las celdas i0..=i1 × j0..=j1, en orden.
    let segmentos = |i0: usize, i1: usize, j0: usize, j1: usize, lista: &mut Vec<Segmento>| {
        lista.clear();
        for b in j0..=j1.min(seg.h - 1) {
            for a in i0..=i1.min(seg.w - 1) {
                for p in &seg.celdas[b * seg.w + a] {
                    let l = &lineas[p.linea as usize];
                    let q = l[(p.idx as usize + 1).min(l.len() - 1)];
                    lista.push(Segmento { a: (p.x, p.y), b: q, linea: p.linea, idx: p.idx, celda: (a, b) });
                }
            }
        }
    };
    // Por bloques de B × B muestras: en el centro del bloque se buscan d1 y d2, y para
    // las muestras del bloque solo cuentan los segmentos a menos de d2 + 2·radio del
    // centro (los demás quedan más lejos que la segunda línea: no pueden ganar).
    const B: usize = 8;
    let radio = paso * (B - 1) as f32 * std::f32::consts::FRAC_1_SQRT_2;
    let filas_b = ch.div_ceil(B);
    v.par_chunks_mut(cw * B).enumerate().for_each(|(jb, bloque)| {
        let (mut todos, mut cerca_c) = (Vec::new(), Vec::new());
        let filas = bloque.len() / cw;
        for i0 in (0..cw).step_by(B) {
            let cols = B.min(cw - i0);
            let (j0, i1, j1) = (jb * B, i0 + cols - 1, jb * B + filas - 1);
            let (a0, b0) = seg.celda(i0 as f32 * paso, j0 as f32 * paso);
            let (a1, b1) = seg.celda(i1 as f32 * paso, j1 as f32 * paso);
            segmentos(a0.saturating_sub(1), a1 + 1, b0.saturating_sub(1), b1 + 1, &mut todos);
            let (xc, yc) = ((i0 + i1) as f32 * paso / 2.0, (j0 + j1) as f32 * paso / 2.0);
            let (_, d2c) = mas_cercanas(xc, yc, &mut todos.iter());
            let lista = if d2c < f32::MAX {
                let tope = (d2c.sqrt() + 2.0 * radio).powi(2);
                cerca_c.clear();
                cerca_c.extend(todos.iter().filter(|s| dist2_seg(xc, yc, s.a.0, s.a.1, s.b.0, s.b.1) <= tope).copied());
                &cerca_c
            } else {
                &todos
            };
            for fj in 0..filas {
                let y = (j0 + fj) as f32 * paso;
                for i in i0..=i1 {
                    let x = i as f32 * paso;
                    let s = sep(x, y);
                    let (d1, d2) = mas_cercanas(x, y, &mut lista.iter());
                    let (d1, d2) = (d1.sqrt().min(2.0 * s), d2.sqrt().min(2.0 * s));
                    // Grosor en mm: el tono es la fracción del hueco entre las dos líneas
                    // que lleva tinta, sin bajar del muro mínimo ni dejar un canal menor
                    // que el mínimo. El campo es una distancia (mm): bordes lisos.
                    let hueco = d1 + d2;
                    // Tono fino (no la rejilla gruesa): el grosor cambia a lo largo de la
                    // línea con el detalle de la imagen, como en el grabado.
                    let t = tono_en(x, y);
                    let medio = (t * hueco / 2.0).max(op.muro_min / 2.0);
                    let tope = ((hueco - op.canal_min) / 2.0).max(op.muro_min / 2.0).min(op.grosor_max / 2.0);
                    bloque[fj * cw + i] = d1 - medio.min(tope);
                }
            }
        }
        let n = hechas.fetch_add(1, Ordering::Relaxed);
        ctl.avance.store((800 + n * 200 / filas_b) as u32, Ordering::Relaxed);
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

/// Une las parejas de extremos de líneas de la misma zona que están a menos de dos
/// separaciones y se miran de frente (cada una apunta a la otra), con un puente de
/// Hermite que sigue las dos tangentes. Se repite mientras haya parejas.
fn unir_extremos(mut lineas: Vec<Vec<(f32, f32)>>, zona: &[u32], sep: &impl Fn(f32, f32) -> f32, paso: f32) -> Vec<Vec<(f32, f32)>> {
    let mut zona = zona.to_vec();
    // Extremo: (línea, ¿es el final?, punto, tangente hacia fuera normalizada).
    let extremos = |lineas: &[Vec<(f32, f32)>]| {
        let mut v = vec![];
        for (k, l) in lineas.iter().enumerate() {
            let n = l.len();
            if n < 4 {
                continue;
            }
            for (fin, p, q) in [(false, l[0], l[3]), (true, l[n - 1], l[n - 4])] {
                let (tx, ty) = (p.0 - q.0, p.1 - q.1);
                let t = tx.hypot(ty).max(1e-6);
                v.push((k, fin, p, (tx / t, ty / t)));
            }
        }
        v
    };
    loop {
        let ext = extremos(&lineas);
        let mut parejas = vec![];
        for (i, &(k, _, p, t)) in ext.iter().enumerate() {
            for (j, &(k2, _, q, u)) in ext.iter().enumerate().skip(i + 1) {
                if k2 == k || zona[k] != zona[k2] {
                    continue;
                }
                let (dx, dy) = (q.0 - p.0, q.1 - p.1);
                let dl = dx.hypot(dy);
                if dl > 2.0 * sep(p.0, p.1) || dl < 1e-6 {
                    continue;
                }
                let (ca, cb) = ((t.0 * dx + t.1 * dy) / dl, -(u.0 * dx + u.1 * dy) / dl);
                if ca > 0.7 && cb > 0.7 {
                    parejas.push((dl / sep(p.0, p.1) * (3.0 - ca - cb), i, j));
                }
            }
        }
        if parejas.is_empty() {
            return lineas;
        }
        parejas.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut usada = vec![false; lineas.len()];
        let mut quitar = vec![];
        for (_, i, j) in parejas {
            let ((ka, fin_a, p, t), (kb, fin_b, q, u)) = (ext[i], ext[j]);
            if usada[ka] || usada[kb] {
                continue;
            }
            (usada[ka], usada[kb]) = (true, true);
            let mut a = std::mem::take(&mut lineas[ka]);
            let mut b = std::mem::take(&mut lineas[kb]);
            if !fin_a {
                a.reverse();
            }
            if fin_b {
                b.reverse();
            }
            let dl = (q.0 - p.0).hypot(q.1 - p.1);
            let n = (dl / paso).ceil() as usize;
            for s in 1..n {
                let x = s as f32 / n as f32;
                let (h00, h10, h01, h11) = (2.0 * x.powi(3) - 3.0 * x * x + 1.0, x.powi(3) - 2.0 * x * x + x, -2.0 * x.powi(3) + 3.0 * x * x, x.powi(3) - x * x);
                a.push((h00 * p.0 + h10 * dl * t.0 + h01 * q.0 - h11 * dl * u.0, h00 * p.1 + h10 * dl * t.1 + h01 * q.1 - h11 * dl * u.1));
            }
            a.extend(b);
            lineas[ka] = a;
            quitar.push(kb);
        }
        quitar.sort_unstable();
        for k in quitar.into_iter().rev() {
            lineas.remove(k);
            zona.remove(k);
        }
    }
}

/// (radio de suavizado en celdas de medio periodo, peso) de cada escala del tensor.
/// Pruebas: DEDALO_ESCALAS="r:p,r:p,…".
fn escalas_tensor() -> Vec<(usize, f32)> {
    std::env::var("DEDALO_ESCALAS")
        .ok()
        .map(|v| v.split(',').filter_map(|e| e.split_once(':').and_then(|(r, p)| Some((r.parse().ok()?, p.parse().ok()?)))).collect())
        .unwrap_or_else(|| vec![(1, 1.0), (3, 0.8), (8, 0.6), (20, 0.5)])
}

/// Suma al tensor `t` (xx, xy, yy) el de un «gradiente» `n` con `peso`.
fn sumar(t: &mut [f32; 3], peso: f32, n: (f32, f32)) {
    t[0] += peso * n.0 * n.0;
    t[1] += peso * n.0 * n.1;
    t[2] += peso * n.1 * n.1;
}

fn suave_paso(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Media móvil [¼ ½ ¼] (`pasadas` veces) sin mover los extremos.
/// Curva de Catmull-Rom por los puntos de `l`, muestreada cada `paso` (aprox.).
fn curva_suave(l: &[(f32, f32)], paso: f32) -> Vec<(f32, f32)> {
    let n = l.len();
    if n < 2 {
        return l.to_vec();
    }
    let p = |i: isize| l[i.clamp(0, n as isize - 1) as usize];
    let mut o = vec![l[0]];
    for i in 0..n as isize - 1 {
        let (p0, p1, p2, p3) = (p(i - 1), p(i), p(i + 1), p(i + 2));
        let m = ((p2.0 - p1.0).hypot(p2.1 - p1.1) / paso).ceil().max(1.0) as usize;
        for k in 1..=m {
            let t = k as f32 / m as f32;
            let (t2, t3) = (t * t, t * t * t);
            let f = |a: f32, b: f32, c: f32, d: f32| 0.5 * (2.0 * b + (c - a) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (3.0 * b - a - 3.0 * c + d) * t3);
            o.push((f(p0.0, p1.0, p2.0, p3.0), f(p0.1, p1.1, p2.1, p3.1)));
        }
    }
    o
}

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
        let c = flujo(100.0, 50.0, 4.0, 3, Some(&tono), ajuste, Opciones { zonas: 1, sigue: 0.0, canal_min: 1.0, muro_min: 0.4, fondo_vacio: false, grosor_max: f32::MAX }, None, &Control::default()).unwrap();
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
mod retrato {
    use super::*;

    /// Con un rostro, las líneas se ordenan alrededor de los ojos: cerca del iris
    /// hay más líneas (anillos) que lejos.
    #[test]
    fn lineas_a_lo_largo_del_ojo() {
        let e = crate::ejemplos::buscar("lincoln").unwrap();
        let img = image::load_from_memory(e.datos).unwrap();
        let r = crate::rostro::detectar(&img.to_rgb8()).unwrap();
        let (ancho, alto) = (200.0, 200.0 * img.height() as f32 / img.width() as f32);
        let cara = crate::rostro::Cara::new(&r, ancho, alto);
        let tono = Tono::new(&crate::tono::gris(&img), ancho, alto, 5.0);
        let op = Opciones { zonas: 1, sigue: 0.5, canal_min: 0.4, muro_min: 0.15, fondo_vacio: false, grosor_max: f32::MAX };
        let o = cara.ojos[0];
        let a_ojo = o.ang;
        let paralelas = |cara: Option<&Cara>| {
            let c = flujo(ancho, alto, 5.0, 1, Some(&tono), Ajuste::default(), op, cara, &Control::default()).unwrap();
            // Junto al ojo (encima y debajo), el campo crece en perpendicular a las líneas:
            // si siguen los párpados, su gradiente es perpendicular al eje del ojo.
            let (mut suma, mut n) = (0.0, 0);
            for k in -20..=20 {
                for lado in [-1.0f32, 1.0] {
                    let t = k as f32 / 20.0 * 0.9 * o.a;
                    let h = lado * 0.5 * o.a;
                    let (x, y) = (o.c.0 + t * a_ojo.cos() - h * a_ojo.sin(), o.c.1 + t * a_ojo.sin() + h * a_ojo.cos());
                    let (i, j) = ((x / c.paso).round() as usize, (y / c.paso).round() as usize);
                    let (gx, gy) = (c.at(i + 2, j) - c.at(i - 2, j), c.at(i, j + 2) - c.at(i, j - 2));
                    let g = gx.hypot(gy);
                    if g > 1e-4 {
                        suma += ((-a_ojo.sin() * gx + a_ojo.cos() * gy) / g).abs();
                        n += 1;
                    }
                }
            }
            suma / n as f32
        };
        let (con, sin) = (paralelas(Some(&cara)), paralelas(None));
        assert!(con > 0.97 && con > sin, "junto al ojo: {con:.2} con retrato, {sin:.2} sin él (1 = líneas a lo largo del ojo)");
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
        let op = Opciones { zonas: 1, sigue: 0.0, canal_min: 1.2, muro_min: 0.4, fondo_vacio: false, grosor_max: f32::MAX };
        let c = flujo(60.0, 60.0, 4.0, 1, Some(&tono), Ajuste::default(), op, None, &Control::default()).unwrap();
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
            let muro: f32 = std::env::var("DEDALO_MURO").ok().and_then(|v| v.parse().ok()).unwrap_or(0.4);
            let canal: f32 = std::env::var("DEDALO_CANAL").ok().and_then(|v| v.parse().ok()).unwrap_or(1.2);
            let op = Opciones { zonas, sigue, canal_min: canal, muro_min: muro, fondo_vacio: false, grosor_max: f32::MAX };
            let t = std::time::Instant::now();
            // DEDALO_RETRATO=0: sin tratamiento de rostro.
            let cara = if std::env::var("DEDALO_RETRATO").is_ok_and(|v| v == "0") { None } else { crate::rostro::detectar(&image::open(&p).unwrap().to_rgb8()).map(|r| crate::rostro::Cara::new(&r, ancho, alto)) };
            println!("rostro: {}", cara.is_some());
            let c = flujo(ancho, alto, periodo, 1, Some(&tono), Ajuste::default(), op, cara.as_ref(), &Control::default()).unwrap();
            let u = Umbrales::new(&c);
            let d = Dibujo { campo: &c, umbrales: &u, tono: Some(&tono), ajuste: Ajuste::default(), limites: Limites::new(periodo, muro, canal) };
            let py = 900usize;
            let px = (py as f32 * ancho / alto) as usize;
            let r = d.raster(px, py, None);
            let dib = image::RgbaImage::from_raw(px as u32, py as u32, r).unwrap();
            let orig = image::imageops::resize(&image::open(&p).unwrap().to_rgba8(), px as u32, py as u32, image::imageops::FilterType::Triangle);
            let mut orig = orig;
            // DEDALO_GUIAS=1: las guías (azul) y los trazos (rojo) del rostro sobre el original.
            if let (Some(c), true) = (&cara, std::env::var("DEDALO_GUIAS").is_ok()) {
                let k = px as f32 / ancho;
                for (ls, color) in [(&c.guias, [0, 90, 255, 255]), (&c.trazos, [255, 0, 0, 255])] {
                    for l in ls.iter() {
                        for w in l.windows(2) {
                            for s in 0..=40 {
                                let t = s as f32 / 40.0;
                                let (x, y) = ((w[0].0 + (w[1].0 - w[0].0) * t) * k, (w[0].1 + (w[1].1 - w[0].1) * t) * k);
                                if x >= 0.0 && y >= 0.0 && (x as u32) < px as u32 && (y as u32) < py as u32 {
                                    orig.put_pixel(x as u32, y as u32, image::Rgba(color));
                                }
                            }
                        }
                    }
                }
            }
            let mut hoja = image::RgbaImage::from_pixel(px as u32 * 2 + 20, py as u32, image::Rgba([255, 255, 255, 255]));
            image::imageops::overlay(&mut hoja, &orig, 0, 0);
            image::imageops::overlay(&mut hoja, &dib, px as i64 + 20, 0);
            let nombre = p.file_stem().unwrap().to_string_lossy().into_owned();
            hoja.save(format!("{sal}/{nombre}.png")).unwrap();
            println!("{nombre}: {:?}", t.elapsed());
        }
    }
}

#[cfg(test)]
mod tiempo {
    use super::*;

    /// Cuánto tarda un retrato a toda la hoja carta (separación 4 mm, con rostro).
    #[test]
    #[ignore]
    fn tiempo_retrato_carta() {
        let img = image::load_from_memory(crate::ejemplos::buscar("lincoln").unwrap().datos).unwrap();
        let (ancho, alto) = (139.5, 232.0);
        let tono = Tono::new(&crate::tono::gris(&img), ancho, alto, 4.0);
        let cara = crate::rostro::detectar(&img.to_rgb8()).map(|r| crate::rostro::Cara::new(&r, ancho, alto));
        let op = Opciones { zonas: 1, sigue: 0.5, canal_min: 1.2, muro_min: 0.4, fondo_vacio: false, grosor_max: f32::MAX };
        let t = std::time::Instant::now();
        let c = flujo(ancho, alto, 4.0, 1, Some(&tono), Ajuste::default(), op, cara.as_ref(), &Control::default()).unwrap();
        println!("{}×{} muestras: {:?}", c.w, c.h, t.elapsed());
    }
}
