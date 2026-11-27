//! Configuración de dedalo: idioma, tema y los parámetros del dibujo, en
//! `<config>/sesion.toml`.

use crate::patron::Tipo;
use crate::salida::{Impresion, Tamano};
use crate::tono::Ajuste;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

/// Carpeta de configuración: `~/Library/Application Support/dedalo` (macOS),
/// `%APPDATA%\dedalo` (Windows) o `~/.config/dedalo`.
pub fn config_dir() -> Option<PathBuf> {
    let env = |k| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        env("HOME").map(|h| h.join("Library/Application Support"))
    } else if cfg!(windows) {
        env("APPDATA")
    } else {
        env("XDG_CONFIG_HOME").or_else(|| env("HOME").map(|h| h.join(".config")))
    };
    Some(base?.join("dedalo"))
}

/// Todo lo que decide el dibujo (se recuerda entre sesiones).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Parametros {
    #[serde(rename = "patron")]
    pub tipo: Tipo,
    /// Centro de un canal al centro del siguiente (mm).
    #[serde(rename = "separacion_mm")]
    pub periodo: f32,
    /// Lo más fino que puede quedar un muro (mm).
    #[serde(rename = "muro_minimo_mm")]
    pub muro_min: f32,
    /// Lo más estrecho que puede quedar un canal (mm): que quepa la pluma.
    #[serde(rename = "canal_minimo_mm")]
    pub canal_min: f32,
    #[serde(rename = "semilla")]
    pub semilla: u64,
    #[serde(rename = "tamano")]
    pub tamano: Tamano,
    #[serde(rename = "cuadro_cm")]
    pub cuadro_cm: f32,
    #[serde(rename = "margen_mm")]
    pub margen: f32,
    #[serde(rename = "impresion")]
    pub impresion: Impresion,
    /// Grosor del contorno (mm).
    #[serde(rename = "linea_mm")]
    pub linea: f32,
    /// Gris del muro en la guía (0 = negro, 1 = blanco).
    #[serde(rename = "gris")]
    pub gris: f32,
    #[serde(rename = "tono", default)]
    pub ajuste: Ajuste,
    /// Líneas de flujo: bandas de tono, cada una con sus líneas (1 = sin zonas).
    #[serde(rename = "zonas", default = "una")]
    pub zonas: u32,
    /// Líneas de flujo: cuánto siguen los bordes de la imagen (0–1).
    #[serde(rename = "siguen_la_imagen", default = "medio")]
    pub sigue: f32,
    /// Líneas de flujo: sin líneas en el fondo liso y claro.
    #[serde(rename = "fondo_vacio", default)]
    pub fondo_vacio: bool,
    /// Líneas de flujo: tratamiento de retrato (ojos, cejas, nariz, boca, óvalo y pelo) si hay un rostro.
    #[serde(rename = "retrato", default = "si")]
    pub retrato: bool,
    /// Líneas de flujo y espiral: grosor máximo de una línea (mm; `GROSOR_SIN_LIMITE` = sin límite).
    #[serde(rename = "grosor_maximo_mm", default = "sin_limite")]
    pub grosor_max: f32,
}

/// En el deslizador de «Grosor máximo», el tope significa «sin límite».
pub const GROSOR_SIN_LIMITE: f32 = 3.0;

fn sin_limite() -> f32 {
    GROSOR_SIN_LIMITE
}

/// Grosor máximo que se usa al dibujar.
pub fn grosor_real(g: f32) -> f32 {
    if g >= GROSOR_SIN_LIMITE { f32::MAX } else { g }
}

fn si() -> bool {
    true
}

fn una() -> u32 {
    1
}

fn medio() -> f32 {
    0.5
}

impl Default for Parametros {
    fn default() -> Parametros {
        Parametros {
            tipo: Tipo::Flujo,
            periodo: 4.0,
            muro_min: 0.4,
            canal_min: 1.2,
            semilla: 1,
            tamano: Tamano::Hoja,
            cuadro_cm: 15.0,
            margen: 15.0,
            impresion: Impresion::Guia,
            linea: 0.2,
            gris: 0.85,
            ajuste: Ajuste::default(),
            zonas: 1,
            sigue: 0.5,
            fondo_vacio: false,
            retrato: true,
            grosor_max: GROSOR_SIN_LIMITE,
        }
    }
}

/// Grupo de ajustes del panel que se puede restablecer por separado.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seccion {
    Imagen,
    Patron,
    Hoja,
    Impresion,
}

impl Parametros {
    /// Los mismos parámetros con los de `s` de fábrica. En «Patrón» se conservan el
    /// tipo y el trazado (semilla) y solo se tocan los ajustes que ese tipo usa.
    pub fn restablecida(self, s: Seccion) -> Parametros {
        let d = Parametros::default();
        let mut p = self;
        match s {
            Seccion::Imagen => p.ajuste = d.ajuste,
            Seccion::Patron => {
                (p.periodo, p.muro_min, p.canal_min, p.grosor_max) = (d.periodo, d.muro_min, d.canal_min, d.grosor_max);
                if p.tipo == Tipo::Flujo {
                    (p.zonas, p.sigue, p.fondo_vacio, p.retrato) = (d.zonas, d.sigue, d.fondo_vacio, d.retrato);
                }
            }
            Seccion::Hoja => (p.tamano, p.cuadro_cm, p.margen) = (d.tamano, d.cuadro_cm, d.margen),
            Seccion::Impresion => (p.impresion, p.linea, p.gris) = (d.impresion, d.linea, d.gris),
        }
        p
    }

    /// Valores dentro de lo razonable (el archivo se puede editar a mano).
    pub fn sano(mut self) -> Parametros {
        // Por debajo de 2 mm solo líneas de flujo y espiral (las rayas serían enormes de calcular).
        self.periodo = self.periodo.clamp(if matches!(self.tipo, Tipo::Flujo | Tipo::Espiral) { 1.5 } else { 2.0 }, 20.0);
        self.grosor_max = self.grosor_max.clamp(0.2, GROSOR_SIN_LIMITE);
        self.muro_min = self.muro_min.clamp(0.1, self.periodo * 0.45);
        self.canal_min = self.canal_min.clamp(0.3, self.periodo * 0.8);
        self.cuadro_cm = self.cuadro_cm.clamp(3.0, 27.0);
        self.margen = self.margen.clamp(0.0, 50.0);
        self.linea = self.linea.clamp(0.05, 1.0);
        self.gris = self.gris.clamp(0.0, 1.0);
        self.zonas = self.zonas.clamp(1, 8);
        self.sigue = self.sigue.clamp(0.0, 1.0);
        self.ajuste.contraste = self.ajuste.contraste.clamp(0.3, 3.0);
        self.ajuste.brillo = self.ajuste.brillo.clamp(-1.0, 1.0);
        self
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Session {
    #[serde(rename = "idioma", default, skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    #[serde(rename = "tema", default, skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,
    /// Avisar de versiones nuevas (mira GitHub al abrir y cada 6 horas); `None` = sí.
    #[serde(rename = "buscar_actualizaciones", default, skip_serializing_if = "Option::is_none")]
    pub check_updates: Option<bool>,
    /// Última imagen abierta (se vuelve a abrir al arrancar si sigue ahí).
    #[serde(rename = "imagen", default, skip_serializing_if = "Option::is_none")]
    pub image: Option<PathBuf>,
    /// O el retrato de ejemplo abierto (`ejemplos::EJEMPLOS`).
    #[serde(rename = "ejemplo", default, skip_serializing_if = "Option::is_none")]
    pub sample: Option<String>,
    /// Carpeta donde se guardó el último PDF/SVG.
    #[serde(rename = "carpeta_salida", default, skip_serializing_if = "Option::is_none")]
    pub out_dir: Option<PathBuf>,
    #[serde(rename = "dibujo", default)]
    pub params: Option<Parametros>,
}

fn session_path() -> Option<PathBuf> {
    Some(config_dir()?.join("sesion.toml"))
}

pub fn load_session() -> Session {
    session_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
}

pub fn save_session(s: &Session) -> io::Result<()> {
    let p = session_path().ok_or_else(|| io::Error::other("sin carpeta de configuración"))?;
    write_atomic(&p, toml::to_string_pretty(s).map_err(io::Error::other)?.as_bytes())
}

/// Escribe en un temporal y renombra: nunca queda un archivo a medias.
pub fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp-dedalo");
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sesion_ida_y_vuelta() {
        let s = Session { lang: Some("es".into()), params: Some(Parametros { tipo: Tipo::Laberinto, ..Default::default() }), ..Default::default() };
        let t = toml::to_string_pretty(&s).unwrap();
        assert!(t.contains("patron = \"laberinto\""));
        assert_eq!(toml::from_str::<Session>(&t).unwrap(), s);
    }

    #[test]
    fn restablecer_por_seccion() {
        let mut p = Parametros { tipo: Tipo::Laberinto, semilla: 7, periodo: 9.0, zonas: 3, margen: 30.0, linea: 0.5, ..Default::default() };
        p.ajuste.brillo = 0.4;
        let r = p.restablecida(Seccion::Patron);
        assert_eq!((r.tipo, r.semilla, r.periodo), (Tipo::Laberinto, 7, 4.0));
        assert_eq!(r.zonas, 3, "las zonas son de las líneas de flujo: el laberinto no las toca");
        assert_eq!((r.margen, r.linea, r.ajuste.brillo), (30.0, 0.5, 0.4), "las otras secciones no cambian");
        assert_eq!(p.restablecida(Seccion::Imagen).ajuste, Ajuste::default());
        assert_eq!(p.restablecida(Seccion::Hoja).margen, 15.0);
        assert_eq!(p.restablecida(Seccion::Impresion).linea, 0.2);
        let f = Parametros { zonas: 3, ..Default::default() }.restablecida(Seccion::Patron);
        assert_eq!(f, Parametros::default());
    }
}
