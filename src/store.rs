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
}

impl Default for Parametros {
    fn default() -> Parametros {
        Parametros {
            tipo: Tipo::Reaccion,
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
        }
    }
}

impl Parametros {
    /// Valores dentro de lo razonable (el archivo se puede editar a mano).
    pub fn sano(mut self) -> Parametros {
        self.periodo = self.periodo.clamp(2.0, 20.0);
        self.muro_min = self.muro_min.clamp(0.1, self.periodo * 0.45);
        self.canal_min = self.canal_min.clamp(0.3, self.periodo * 0.8);
        self.cuadro_cm = self.cuadro_cm.clamp(3.0, 27.0);
        self.margen = self.margen.clamp(0.0, 50.0);
        self.linea = self.linea.clamp(0.05, 1.0);
        self.gris = self.gris.clamp(0.0, 1.0);
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
}
