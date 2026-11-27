// En Windows, sin ventana de consola junto a la app.
#![cfg_attr(windows, windows_subsystem = "windows")]
// Primero: sus macros t!/tf! se usan en todos los demás módulos.
#[macro_use]
mod i18n;
mod app;
mod contorno;
mod ejemplos;
mod espiral;
mod estilo;
mod flujo;
mod icon;
mod icons;
mod patron;
mod rostro;
mod salida;
mod store;
mod tono;
mod update;

/// Identidad de dedalo para los módulos comunes (logotipo, «Acerca de»…).
pub const APP: icon::Marca = icon::Marca {
    nombre: "dedalo",
    descripcion: || t!("Laberintos para rellenar a mano que forman una imagen"),
    letra: icon::LETRA_D,
    color: icon::ROJO,
    hecho_con: "Rust · egui / eframe · image · rayon",
};

fn main() -> eframe::Result {
    let args: Vec<std::path::PathBuf> = std::env::args_os().skip(1).map(Into::into).collect();
    // Empaquetado: `dedalo --iconset carpeta` (macOS) y `dedalo --ico archivo.ico` (Windows).
    match args.first().and_then(|a| a.to_str()) {
        Some("--iconset") => {
            let dir = args.get(1).cloned().unwrap_or_else(|| "dedalo.iconset".into());
            icon::write_iconset(&dir).expect("no se pudo escribir el iconset");
            return Ok(());
        }
        Some("--ico") => {
            let path = args.get(1).cloned().unwrap_or_else(|| "dedalo.ico".into());
            std::fs::write(&path, icon::ico()).expect("no se pudo escribir el .ico");
            return Ok(());
        }
        _ => {}
    }
    // `dedalo imagen`: abre esa imagen.
    let open = args.first().filter(|p| p.is_file()).cloned();
    // En modo captura (pruebas) la ventana no se activa: no roba el teclado.
    let testing = std::env::var_os("DEDALO_SCREENSHOT").is_some();
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_active(!testing)
            .with_title("dedalo")
            .with_app_id("dedalo")
            .with_inner_size([1200.0, 800.0])
            .with_min_inner_size([820.0, 560.0])
            .with_drag_and_drop(true)
            .with_icon(std::sync::Arc::new(icon::app_icon())),
        persist_window: !testing,
        ..Default::default()
    };
    estilo::arrancar("dedalo", options, Box::new(move |cc| Ok(Box::new(app::App::new(cc, open)))))
}
