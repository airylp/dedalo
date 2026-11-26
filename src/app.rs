//! Interfaz de dedalo: panel de ajustes a la izquierda y la hoja en el centro.
//!
//! El patrón (lo lento: la simulación de reacción-difusión o el laberinto) solo
//! se recalcula si cambia su forma (tipo, tamaño, separación, semilla); el tono
//! de la imagen y el estilo de impresión se aplican encima al momento.

use crate::contorno::{Dibujo, Limites};
use crate::icons::{Icon, IconButton, Tone};
use crate::patron::{Campo, Control, Tipo, Umbrales};
use crate::salida::{Impresion, Pagina, Tamano, Trazo};
use crate::store::{self, Parametros, Session};
use crate::tono::Tono;
use eframe::egui::{
    self, Align, Align2, CentralPanel, Color32, ColorImage, Context, Key, Layout, Modifiers, Panel, RawInput, Rect, RichText, ScrollArea, Sense, Slider,
    Stroke, TextureHandle, TextureOptions, Ui, ViewportCommand, pos2, vec2,
};
use image::GrayImage;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Imagen abierta.
struct Imagen {
    path: PathBuf,
    gris: Arc<GrayImage>,
    /// Ancho / alto.
    aspecto: f32,
    /// En color, reducida, para «Ver la imagen».
    textura: TextureHandle,
}

/// Lo que decide la forma del patrón (si cambia, hay que recalcularlo).
#[derive(Clone, Copy, PartialEq, Debug)]
struct ClaveCampo {
    tipo: Tipo,
    ancho: f32,
    alto: f32,
    periodo: f32,
    semilla: u64,
    /// Solo líneas de flujo (dependen de la imagen): imagen, ajuste, zonas, cuánto siguen.
    imagen: usize,
    ajuste: Option<crate::tono::Ajuste>,
    zonas: u32,
    sigue: f32,
    canal_min: f32,
    muro_min: f32,
}

struct Patron {
    clave: ClaveCampo,
    campo: Arc<Campo>,
    umbrales: Arc<Umbrales>,
    /// Cambia con cada patrón nuevo (para las vistas).
    id: u64,
}

struct TrabajoPatron {
    clave: ClaveCampo,
    ctl: Arc<Control>,
    rx: Receiver<Option<(Arc<Campo>, Arc<Umbrales>)>>,
    inicio: Instant,
}

/// Lo que decide el contenido de la vista previa.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ClaveVista {
    patron: u64,
    imagen: usize,
    periodo: f32,
    ajuste: crate::tono::Ajuste,
    muro_min: f32,
    canal_min: f32,
    impresion: Option<(Impresion, f32, f32)>,
}

/// Un trozo dibujado de la vista previa: qué rectángulo (mm del dibujo) y a
/// qué resolución (píxeles por mm).
#[derive(Clone, Copy, PartialEq, Debug)]
struct Trozo {
    clave: ClaveVista,
    region: Rect,
    px: usize,
    py: usize,
}

impl Trozo {
    fn resolucion(&self) -> f32 {
        self.px as f32 / self.region.width().max(1e-3)
    }
}

struct TrabajoVista {
    trozo: Trozo,
    rx: Receiver<(Vec<u8>, Option<Arc<Tono>>)>,
}

/// Tono ya calculado: imagen (puntero) + área + separación.
type ClaveTono = (usize, u32, u32, u32);

#[derive(Clone, Copy, PartialEq)]
enum Ver {
    /// Muros rellenos: lo que se verá al terminar de rellenar.
    Terminado,
    /// Como sale de la impresora.
    Impresion,
    /// La imagen original.
    Original,
}

enum Destino {
    Imagen,
    Pdf,
    Svg,
}

struct Eleccion {
    destino: Destino,
    result: Arc<Mutex<Option<Option<PathBuf>>>>,
}

pub struct App {
    session: Session,
    session_dirty: bool,
    last_session_save: Instant,
    p: Parametros,
    imagen: Option<Imagen>,
    patron: Option<Patron>,
    trabajo: Option<TrabajoPatron>,
    /// Cuándo cambió por última vez la forma pedida (se espera un poco antes de recalcular).
    cambio: Instant,
    ultima: Option<ClaveCampo>,
    ids: u64,
    tono: Option<(ClaveTono, Arc<Tono>)>,
    vista: Option<(Trozo, TextureHandle)>,
    /// Zoom de la hoja (1 = cabe entera) y punto de la hoja (mm) en el centro de la vista.
    zoom: f32,
    centro: Option<egui::Pos2>,
    trabajo_vista: Option<TrabajoVista>,
    ver: Ver,
    solucion: bool,
    eleccion: Option<Eleccion>,
    exportando: Option<Receiver<Result<PathBuf, String>>>,
    logo: Option<TextureHandle>,
    show_settings: bool,
    show_about: bool,
    show_keys: bool,
    updater: Option<crate::update::Updater>,
    show_updater: bool,
    vigia: crate::estilo::Vigia,
    avisos: crate::estilo::Avisos,
    screenshot: Option<String>,
    frames: u32,
}

/// Zoom máximo (veces la hoja entera).
const MAX_ZOOM: f32 = 40.0;

fn bits(x: f32) -> u32 {
    x.to_bits()
}

impl App {
    pub fn new(cc: &eframe::CreationContext, open: Option<PathBuf>) -> App {
        let screenshot = std::env::var("DEDALO_SCREENSHOT").ok();
        let session = if screenshot.is_some() { Session::default() } else { store::load_session() };
        if let Some(l) = session.lang.as_deref().and_then(crate::i18n::Lang::from_code).or_else(crate::i18n::system) {
            crate::i18n::set(l);
        }
        if let Some(l) = std::env::var("DEDALO_IDIOMA").ok().as_deref().and_then(crate::i18n::Lang::from_code) {
            crate::i18n::set(l);
        }
        crate::estilo::aplicar_tema(&cc.egui_ctx, crate::estilo::Tema::de_clave(session.theme.as_deref().unwrap_or("")));
        crate::estilo::aplicar(&cc.egui_ctx);
        if let Ok(t) = std::env::var("DEDALO_TEMA") {
            crate::estilo::aplicar_tema(&cc.egui_ctx, crate::estilo::Tema::de_clave(&t));
        }
        let mut p = session.params.unwrap_or_default().sano();
        match std::env::var("DEDALO_PATRON").as_deref() {
            Ok("laberinto") => p.tipo = Tipo::Laberinto,
            Ok("reaccion") => p.tipo = Tipo::Reaccion,
            Ok("flujo") => p.tipo = Tipo::Flujo,
            _ => {}
        }
        let mut app = App {
            p,
            session,
            session_dirty: false,
            last_session_save: Instant::now(),
            imagen: None,
            patron: None,
            trabajo: None,
            cambio: Instant::now() - Duration::from_secs(1),
            ultima: None,
            ids: 0,
            tono: None,
            vista: None,
            zoom: std::env::var("DEDALO_ZOOM").ok().and_then(|z| z.parse().ok()).unwrap_or(1.0),
            centro: None,
            trabajo_vista: None,
            ver: match std::env::var("DEDALO_VER").as_deref() {
                Ok("impresion") => Ver::Impresion,
                Ok("original") => Ver::Original,
                _ => Ver::Terminado,
            },
            solucion: std::env::var_os("DEDALO_SOLUCION").is_some(),
            eleccion: None,
            exportando: None,
            logo: None,
            show_settings: false,
            show_about: false,
            show_keys: false,
            updater: None,
            show_updater: false,
            vigia: Default::default(),
            avisos: Default::default(),
            screenshot,
            frames: 0,
        };
        let inicial = open.or_else(|| std::env::var_os("DEDALO_IMAGEN").map(PathBuf::from)).or_else(|| app.session.image.clone().filter(|p| p.is_file()));
        if let Some(path) = inicial {
            app.abrir(&cc.egui_ctx, &path);
        }
        app
    }

    fn abrir(&mut self, ctx: &Context, path: &Path) {
        match crate::tono::abrir(path) {
            Ok(gris) => {
                let aspecto = gris.width() as f32 / gris.height().max(1) as f32;
                // Miniatura en color para «Ver la imagen».
                let color = image::open(path).map(|i| i.thumbnail(1400, 1400).to_rgba8()).unwrap_or_default();
                let tex = ctx.load_texture(
                    "imagen",
                    ColorImage::from_rgba_unmultiplied([color.width() as usize, color.height() as usize], color.as_raw()),
                    TextureOptions::LINEAR,
                );
                self.imagen = Some(Imagen { path: path.to_path_buf(), gris: Arc::new(gris), aspecto, textura: tex });
                self.tono = None;
                if self.screenshot.is_none() {
                    self.session.image = Some(path.to_path_buf());
                    self.session_dirty = true;
                }
            }
            Err(e) => self.avisos.error(tf!("No se pudo abrir la imagen: {e}", e = e)),
        }
    }

    fn pagina(&self) -> Pagina {
        let aspecto = self.imagen.as_ref().map_or_else(
            || {
                let (w, h) = crate::salida::CARTA;
                (w - 2.0 * self.p.margen) / (h - 2.0 * self.p.margen)
            },
            |i| i.aspecto,
        );
        Pagina::new(self.p.tamano, self.p.cuadro_cm, self.p.margen, aspecto)
    }

    fn clave_campo(&self) -> ClaveCampo {
        let pag = self.pagina();
        // Al décimo de mm: un cambio mínimo del margen no rehace el patrón.
        let r = |x: f32| (x * 10.0).round() / 10.0;
        let flujo = self.p.tipo == Tipo::Flujo;
        ClaveCampo {
            tipo: self.p.tipo,
            ancho: r(pag.area.0),
            alto: r(pag.area.1),
            periodo: self.p.periodo,
            semilla: self.p.semilla,
            imagen: if flujo { self.imagen.as_ref().map_or(0, |i| Arc::as_ptr(&i.gris) as usize) } else { 0 },
            ajuste: flujo.then_some(self.p.ajuste),
            zonas: if flujo { self.p.zonas } else { 0 },
            sigue: if flujo { self.p.sigue } else { 0.0 },
            canal_min: if flujo { self.p.canal_min } else { 0.0 },
            muro_min: if flujo { self.p.muro_min } else { 0.0 },
        }
    }

    fn limites(&self) -> Limites {
        Limites::new(self.p.periodo, self.p.muro_min, self.p.canal_min)
    }

    fn trazo(&self) -> Trazo {
        Trazo { impresion: self.p.impresion, linea: self.p.linea, gris: self.p.gris }
    }

    /// Lanza, recoge o cancela el cálculo del patrón.
    fn patron_al_dia(&mut self, ctx: &Context) {
        if let Some(t) = &self.trabajo {
            if let Ok(r) = t.rx.try_recv() {
                let t = self.trabajo.take().unwrap();
                if let Some((campo, umbrales)) = r {
                    self.ids += 1;
                    self.patron = Some(Patron { clave: t.clave, campo, umbrales, id: self.ids });
                }
            }
        }
        let quiero = self.clave_campo();
        if self.ultima != Some(quiero) {
            self.ultima = Some(quiero);
            self.cambio = Instant::now();
        }
        if self.patron.as_ref().is_some_and(|p| p.clave == quiero) {
            if let Some(t) = self.trabajo.take() {
                t.ctl.cancelar.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        if let Some(t) = &self.trabajo {
            if t.clave == quiero {
                ctx.request_repaint_after(Duration::from_millis(100));
                return;
            }
            t.ctl.cancelar.store(true, std::sync::atomic::Ordering::Relaxed);
            self.trabajo = None;
            self.cambio = Instant::now();
        }
        // Mientras se arrastra un control, se espera a que pare.
        let espera = Duration::from_millis(350);
        if self.cambio.elapsed() < espera || ctx.input(|i| i.pointer.any_down()) {
            ctx.request_repaint_after(Duration::from_millis(80));
            return;
        }
        let ctl = Arc::new(Control::default());
        let (tx, rx) = channel();
        let (c, k) = (ctl.clone(), quiero);
        let repintar = ctx.clone();
        let gris = self.imagen.as_ref().map(|i| i.gris.clone());
        let opciones = crate::flujo::Opciones { zonas: k.zonas, sigue: k.sigue, canal_min: k.canal_min, muro_min: k.muro_min };
        std::thread::spawn(move || {
            let campo = match k.tipo {
                Tipo::Flujo => {
                    let tono = gris.map(|g| Tono::new(&g, k.ancho, k.alto, k.periodo));
                    crate::flujo::flujo(k.ancho, k.alto, k.periodo, k.semilla, tono.as_ref(), k.ajuste.unwrap_or_default(), opciones, &c)
                }
                Tipo::Reaccion => crate::patron::reaccion_difusion(k.ancho, k.alto, k.periodo, k.semilla, crate::patron::RD_PASOS, &c),
                Tipo::Laberinto => crate::patron::laberinto(k.ancho, k.alto, k.periodo, k.semilla, &c),
            };
            let r = campo.map(|campo| {
                let u = Umbrales::new(&campo);
                (Arc::new(campo), Arc::new(u))
            });
            let _ = tx.send(r);
            repintar.request_repaint();
        });
        self.trabajo = Some(TrabajoPatron { clave: quiero, ctl, rx, inicio: Instant::now() });
    }

    /// Lanza o recoge la vista previa: `visible` = lo que se ve del dibujo
    /// (mm), `res` = píxeles por mm de pantalla. Solo se vuelve a dibujar si lo
    /// visible se sale de lo ya dibujado o si la resolución quedó corta o sobrada.
    fn vista_al_dia(&mut self, ctx: &Context, visible: Rect, ext: Rect, res: f32) {
        if let Some(t) = &self.trabajo_vista {
            if let Ok((rgba, tono)) = t.rx.try_recv() {
                let t = self.trabajo_vista.take().unwrap();
                if let (Some(tn), Some(img)) = (tono, &self.imagen) {
                    self.tono = Some((clave_tono(img, &t.trozo.clave), tn));
                }
                // DEDALO_VOLCADO=archivo.png (pruebas): guarda el último trozo dibujado.
                if let Ok(f) = std::env::var("DEDALO_VOLCADO") {
                    let _ = image::RgbaImage::from_raw(t.trozo.px as u32, t.trozo.py as u32, rgba.clone()).map(|i| i.save(f));
                }
                let tex = ctx.load_texture("vista", ColorImage::from_rgba_unmultiplied([t.trozo.px, t.trozo.py], &rgba), TextureOptions::LINEAR);
                self.vista = Some((t.trozo, tex));
            } else {
                return;
            }
        }
        let Some(pat) = &self.patron else { return };
        let clave = ClaveVista {
            patron: pat.id,
            imagen: self.imagen.as_ref().map_or(0, |i| Arc::as_ptr(&i.gris) as usize),
            periodo: self.p.periodo,
            ajuste: self.p.ajuste,
            muro_min: self.p.muro_min,
            canal_min: self.p.canal_min,
            impresion: (self.ver == Ver::Impresion).then_some((self.p.impresion, self.p.linea, self.p.gris)),
        };
        if visible.width() <= 0.0 || visible.height() <= 0.0 {
            return;
        }
        if let Some((t, _)) = &self.vista {
            let r = t.resolucion();
            let llega = t.region.expand(0.01).contains_rect(visible) || t.region.expand(0.01).contains_rect(ext.intersect(visible));
            if t.clave == clave && llega && r >= res * 0.75 && r <= res * 1.6 {
                return;
            }
        }
        // Lo visible más un margen (para poder mover un poco sin redibujar).
        let margen = vec2(visible.width(), visible.height()) * 0.3;
        let region = visible.expand2(margen).intersect(ext);
        let mut res = res;
        let max = 4096.0;
        if region.width() * res > max || region.height() * res > max {
            res = max / region.width().max(region.height());
        }
        let px = ((region.width() * res) as usize).max(8);
        let py = ((region.height() * res) as usize).max(8);
        let trozo = Trozo { clave, region, px, py };
        let (campo, umbrales) = (pat.campo.clone(), pat.umbrales.clone());
        let tono_hecho = self.imagen.as_ref().and_then(|i| self.tono.as_ref().filter(|(k, _)| *k == clave_tono(i, &clave)).map(|(_, t)| t.clone()));
        let gris = self.imagen.as_ref().map(|i| i.gris.clone());
        let (limites, trazo) = (self.limites(), self.trazo());
        let (tx, rx) = channel();
        let repintar = ctx.clone();
        std::thread::spawn(move || {
            let nuevo = match (&tono_hecho, &gris) {
                (None, Some(g)) => Some(Arc::new(Tono::new(g, campo.ancho, campo.alto, clave.periodo))),
                _ => None,
            };
            let tono = tono_hecho.as_ref().or(nuevo.as_ref());
            let d = Dibujo { campo: &campo, umbrales: &umbrales, tono: tono.map(|t| &**t), ajuste: clave.ajuste, limites };
            let t = clave.impresion.map(|_| trazo);
            let r = (trozo.region.min.x, trozo.region.min.y, trozo.region.max.x, trozo.region.max.y);
            let rgba = d.raster_region(r, trozo.px, trozo.py, t);
            let _ = tx.send((rgba, nuevo));
            repintar.request_repaint();
        });
        self.trabajo_vista = Some(TrabajoVista { trozo, rx });
    }

    fn elegir(&mut self, destino: Destino, ctx: &Context) {
        if self.eleccion.is_some() {
            return;
        }
        type Fut = std::pin::Pin<Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>>;
        let d = rfd::AsyncFileDialog::new();
        let carpeta = match destino {
            Destino::Imagen => self.imagen.as_ref().and_then(|i| i.path.parent().map(Path::to_path_buf)),
            _ => self.session.out_dir.clone().or_else(|| self.imagen.as_ref().and_then(|i| i.path.parent().map(Path::to_path_buf))),
        };
        let d = match &carpeta {
            Some(x) => d.set_directory(x),
            None => d,
        };
        let base = self.imagen.as_ref().and_then(|i| i.path.file_stem()).map_or_else(|| "dedalo".to_string(), |s| s.to_string_lossy().into_owned());
        let fut: Fut = match destino {
            Destino::Imagen => Box::pin(d.set_title(t!("Abrir imagen")).add_filter(t!("Imágenes"), &["png", "jpg", "jpeg", "gif", "webp", "bmp"]).add_filter(t!("Todos"), &["*"]).pick_file()),
            Destino::Pdf => Box::pin(d.set_title(t!("Guardar PDF")).set_file_name(format!("{base}-dedalo.pdf")).add_filter("PDF", &["pdf"]).save_file()),
            Destino::Svg => Box::pin(d.set_title(t!("Guardar SVG")).set_file_name(format!("{base}-dedalo.svg")).add_filter("SVG", &["svg"]).save_file()),
        };
        let result = Arc::new(Mutex::new(None));
        let r = result.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            *r.lock().unwrap() = Some(block_on(fut).map(|h| h.path().to_path_buf()));
            ctx.request_repaint();
        });
        self.eleccion = Some(Eleccion { destino, result });
    }

    fn poll(&mut self, ctx: &Context) {
        if let Some(e) = &self.eleccion {
            let r = e.result.lock().unwrap().take();
            if let Some(r) = r {
                let e = self.eleccion.take().unwrap();
                if let Some(path) = r {
                    match e.destino {
                        Destino::Imagen => self.abrir(ctx, &path),
                        Destino::Pdf => self.exportar(path, true, ctx),
                        Destino::Svg => self.exportar(path, false, ctx),
                    }
                }
            }
        }
        if let Some(rx) = &self.exportando {
            if let Ok(r) = rx.try_recv() {
                self.exportando = None;
                match r {
                    Ok(p) => self.avisos.exito(tf!("✔ Guardado: {p}", p = p.display())),
                    Err(e) => self.avisos.error(tf!("No se pudo guardar: {e}", e = e)),
                }
            }
        }
        let dropped: Vec<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect());
        if let Some(p) = dropped.into_iter().next() {
            self.abrir(ctx, &p);
        }
    }

    /// Contornos y archivo en un hilo.
    fn exportar(&mut self, path: PathBuf, pdf: bool, ctx: &Context) {
        let Some(pat) = &self.patron else {
            self.avisos.error(t!("Espera a que termine de dibujarse el patrón."));
            return;
        };
        if pat.clave != self.clave_campo() {
            self.avisos.error(t!("Espera a que termine de dibujarse el patrón."));
            return;
        }
        if let Some(d) = path.parent() {
            self.session.out_dir = Some(d.to_path_buf());
            self.session_dirty = true;
        }
        let (campo, umbrales) = (pat.campo.clone(), pat.umbrales.clone());
        let gris = self.imagen.as_ref().map(|i| i.gris.clone());
        let tono = self.tono.as_ref().map(|(_, t)| t.clone());
        let (limites, trazo, ajuste, periodo, pag) = (self.limites(), self.trazo(), self.p.ajuste, self.p.periodo, self.pagina());
        let titulo = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let (tx, rx) = channel();
        let repintar = ctx.clone();
        std::thread::spawn(move || {
            let tono = tono.or_else(|| gris.map(|g| Arc::new(Tono::new(&g, campo.ancho, campo.alto, periodo))));
            let d = Dibujo { campo: &campo, umbrales: &umbrales, tono: tono.as_deref(), ajuste, limites };
            let figuras = d.contornos(0.03);
            let origen = pag.origen(campo.ancho, campo.alto);
            let datos = if pdf { crate::salida::pdf(&pag, origen, &figuras, trazo, &titulo) } else { crate::salida::svg(&pag, origen, &figuras, trazo).into_bytes() };
            let r = store::write_atomic(&path, &datos).map(|_| path).map_err(|e| e.to_string());
            let _ = tx.send(r);
            repintar.request_repaint();
        });
        self.exportando = Some(rx);
    }

    fn open_updater(&mut self, ctx: &Context) {
        let running = self.updater.as_ref().is_some_and(|u| u.stage() == crate::update::Stage::Running);
        if !running {
            self.updater = Some(crate::update::Updater::check(ctx.clone()));
        }
        self.show_updater = true;
    }

    fn updater_window(&mut self, ctx: &Context) {
        let Some(u) = &self.updater else { return };
        if !self.show_updater {
            return;
        }
        let mut open = true;
        match crate::estilo::ventana_actualizar(ctx, u, &mut open, None) {
            crate::estilo::AccionActualizar::Ejecutar => u.run(ctx.clone()),
            crate::estilo::AccionActualizar::Reiniciar(p) => match crate::update::relaunch_after_exit(&p) {
                Ok(()) => ctx.send_viewport_cmd(ViewportCommand::Close),
                Err(e) => self.avisos.error(tf!("No se pudo reiniciar: {e}", e = e)),
            },
            crate::estilo::AccionActualizar::Nada => {}
        }
        if !open {
            self.show_updater = false;
        }
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            if ui.add(IconButton::new(Icon::Open, Tone::File).label(t!("Abrir imagen…"))).on_hover_text(crate::estilo::tecla("Ctrl+O")).clicked() {
                self.elegir(Destino::Imagen, &ctx);
            }
            ui.separator();
            let listo = self.patron.as_ref().is_some_and(|p| p.clave == self.clave_campo()) && self.exportando.is_none();
            ui.add_enabled_ui(listo, |ui| {
                if ui.add(IconButton::new(Icon::Download, Tone::File).label(t!("Guardar PDF…")).primary()).on_hover_text(crate::estilo::tecla("Ctrl+S")).clicked() {
                    self.elegir(Destino::Pdf, &ctx);
                }
                if ui.add(IconButton::new(Icon::Save, Tone::File).label(t!("Guardar SVG…"))).on_hover_text(crate::estilo::tecla("Ctrl+Shift+S")).clicked() {
                    self.elegir(Destino::Svg, &ctx);
                }
            });
            if self.exportando.is_some() {
                ui.spinner();
            }
            ui.separator();
            if ui.add(IconButton::new(Icon::Reload, Tone::Tool).label(t!("Otro dibujo"))).on_hover_text(format!("{} · {}", t!("Mismo estilo, otro trazado al azar"), crate::estilo::tecla("Ctrl+R"))).clicked() {
                self.otra_semilla();
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(IconButton::new(Icon::Gear, Tone::Neutral).selected(self.show_settings)).on_hover_text(t!("Ajustes")).clicked() {
                    self.show_settings = !self.show_settings;
                }
                let nueva = self.vigia.nueva(self.updater.as_ref());
                if crate::estilo::boton_actualizar(ui, self.show_updater, nueva.as_deref()) {
                    if nueva.is_some() && self.updater.is_some() {
                        self.show_updater = true;
                    } else if nueva.is_some() {
                        self.updater = self.vigia.comprobacion.take();
                        self.show_updater = true;
                    } else {
                        self.open_updater(&ctx);
                    }
                }
            });
        });
    }

    fn otra_semilla(&mut self) {
        self.p.semilla = self.p.semilla.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) % 1_000_000;
        self.cambio = Instant::now() - Duration::from_secs(1);
        self.session_dirty = true;
    }

    /// Panel izquierdo: imagen, patrón, hoja e impresión.
    fn controles(&mut self, ui: &mut Ui) {
        let antes = self.p;
        use crate::estilo::{fila, fila_bloque, rejilla, seccion};
        ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            seccion(ui, t!("Imagen"));
            match &self.imagen {
                Some(img) => {
                    let nombre = img.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    ui.horizontal(|ui| {
                        let (w, h) = (64.0, 64.0 / img.aspecto.max(0.2)).min_lado(64.0);
                        ui.add(egui::Image::new(&img.textura).fit_to_exact_size(vec2(w, h)).corner_radius(4.0));
                        ui.vertical(|ui| {
                            ui.label(RichText::new(nombre).strong());
                            ui.label(RichText::new(format!("{} × {} px", img.gris.width(), img.gris.height())).small().weak());
                        });
                    });
                }
                None => crate::estilo::alerta(ui, crate::estilo::Tipo::Info, t!("Abre una imagen (o arrástrala aquí). Sin imagen se dibuja un gris parejo.")),
            }
            ui.add_space(4.0);
            rejilla(ui, "imagen", |ui| {
                fila(ui, t!("Brillo"), Some(t!("Más claro ←  → más oscuro")), |ui| {
                    ui.add(Slider::new(&mut self.p.ajuste.brillo, -1.0..=1.0).fixed_decimals(2));
                });
                fila(ui, t!("Contraste"), None, |ui| {
                    ui.add(Slider::new(&mut self.p.ajuste.contraste, 0.3..=3.0).fixed_decimals(2));
                });
                fila(ui, t!("Niveles"), None, |ui| {
                    ui.checkbox(&mut self.p.ajuste.niveles, t!("Automáticos (del blanco al negro)"));
                });
                fila(ui, t!("Negativo"), None, |ui| {
                    ui.checkbox(&mut self.p.ajuste.invertir, t!("Invertir claros y oscuros"));
                });
            });

            seccion(ui, t!("Patrón"));
            rejilla(ui, "patron", |ui| {
                fila_bloque(ui, t!("Tipo"), None, |ui| {
                    ui.vertical(|ui| {
                        ui.radio_value(&mut self.p.tipo, Tipo::Flujo, t!("Líneas de flujo"));
                        ui.radio_value(&mut self.p.tipo, Tipo::Reaccion, t!("Rayas (reacción-difusión)"));
                        ui.radio_value(&mut self.p.tipo, Tipo::Laberinto, t!("Laberinto con salida"));
                    });
                });
                fila(ui, t!("Separación"), Some(t!("De un canal al siguiente: más pequeña = más detalle y más trabajo de relleno")), |ui| {
                    ui.add(Slider::new(&mut self.p.periodo, 2.5..=12.0).suffix(" mm").fixed_decimals(1));
                });
                fila(ui, t!("Muro mínimo"), Some(t!("Lo más fino que queda un muro en las zonas claras")), |ui| {
                    ui.add(Slider::new(&mut self.p.muro_min, 0.1..=(self.p.periodo * 0.45)).suffix(" mm").fixed_decimals(1));
                });
                fila(ui, t!("Canal mínimo"), Some(t!("Lo más estrecho que queda un canal en las zonas oscuras")), |ui| {
                    ui.add(Slider::new(&mut self.p.canal_min, 0.3..=(self.p.periodo * 0.8)).suffix(" mm").fixed_decimals(1));
                });
                fila(ui, t!("Trazado"), None, |ui| {
                    ui.add(egui::DragValue::new(&mut self.p.semilla).range(0..=999_999).prefix("#"));
                    if ui.add(IconButton::new(Icon::Reload, Tone::Tool).small()).on_hover_text(t!("Mismo estilo, otro trazado al azar")).clicked() {
                        self.otra_semilla();
                    }
                });
                if self.p.tipo == Tipo::Flujo {
                    fila(ui, t!("Zonas"), Some(t!("Bandas de tono: cada una se rellena con sus propias líneas, en otra dirección (1 = sin zonas)")), |ui| {
                        ui.add(Slider::new(&mut self.p.zonas, 1..=6));
                    });
                    fila(ui, t!("Siguen la imagen"), Some(t!("0 % = curvas al azar; 100 % = rodean las formas de la imagen")), |ui| {
                        let mut pct = (self.p.sigue * 100.0).round();
                        if ui.add(Slider::new(&mut pct, 0.0..=100.0).suffix(" %").fixed_decimals(0)).changed() {
                            self.p.sigue = pct / 100.0;
                        }
                    });
                }
                if self.p.tipo == Tipo::Laberinto {
                    fila(ui, t!("Solución"), None, |ui| {
                        ui.checkbox(&mut self.solucion, t!("Mostrarla en la vista (no se imprime)"));
                    });
                }
            });

            seccion(ui, t!("Hoja"));
            rejilla(ui, "hoja", |ui| {
                fila_bloque(ui, t!("Tamaño"), None, |ui| {
                    ui.vertical(|ui| {
                        ui.radio_value(&mut self.p.tamano, Tamano::Hoja, t!("Carta completa"));
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut self.p.tamano, Tamano::Cuadro, t!("Cuadro de"));
                            ui.add_enabled(self.p.tamano == Tamano::Cuadro, egui::DragValue::new(&mut self.p.cuadro_cm).range(3.0..=27.0).speed(0.1).suffix(" cm"));
                        });
                    });
                });
                fila(ui, t!("Margen"), None, |ui| {
                    ui.add(Slider::new(&mut self.p.margen, 5.0..=40.0).suffix(" mm").fixed_decimals(0));
                });
            });

            seccion(ui, t!("Impresión"));
            rejilla(ui, "impresion", |ui| {
                fila_bloque(ui, t!("Se imprime"), None, |ui| {
                    ui.vertical(|ui| {
                        ui.radio_value(&mut self.p.impresion, Impresion::Guia, t!("Contorno y muro en gris (guía para rellenar)"));
                        ui.radio_value(&mut self.p.impresion, Impresion::Contorno, t!("Solo el contorno"));
                        ui.radio_value(&mut self.p.impresion, Impresion::Relleno, t!("Muros rellenos (terminado)"));
                    });
                });
                fila(ui, t!("Línea"), None, |ui| {
                    ui.add(Slider::new(&mut self.p.linea, 0.05..=0.8).suffix(" mm").fixed_decimals(2));
                });
                if self.p.impresion == Impresion::Guia {
                    fila(ui, t!("Gris del muro"), Some(t!("Más claro = se nota menos lo que falta por rellenar")), |ui| {
                        let mut pct = ((1.0 - self.p.gris) * 100.0).round();
                        if ui.add(Slider::new(&mut pct, 3.0..=40.0).suffix(" %").fixed_decimals(0)).changed() {
                            self.p.gris = 1.0 - pct / 100.0;
                        }
                    });
                }
            });
            ui.add_space(8.0);
            if ui.button(t!("Valores iniciales")).on_hover_text(t!("Vuelve a los valores de fábrica del dibujo (no cambia la imagen)")).clicked() {
                let semilla = self.p.semilla;
                self.p = Parametros { semilla, ..Default::default() };
            }
            ui.add_space(8.0);
        });
        self.p = self.p.sano();
        if self.p != antes {
            self.session_dirty = true;
            let (a, b) = (antes, self.p);
            if (a.tipo, a.periodo, a.semilla, a.tamano, a.cuadro_cm, a.margen) != (b.tipo, b.periodo, b.semilla, b.tamano, b.cuadro_cm, b.margen) {
                self.cambio = Instant::now();
            }
        }
    }

    /// La hoja con el dibujo. Rueda = acercar/alejar hacia el puntero,
    /// arrastrar = mover, doble clic = ver la hoja entera.
    fn hoja(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("Ver")).weak());
            ui.selectable_value(&mut self.ver, Ver::Terminado, t!("Terminado"));
            ui.selectable_value(&mut self.ver, Ver::Impresion, t!("Como se imprime"));
            ui.add_enabled_ui(self.imagen.is_some(), |ui| {
                ui.selectable_value(&mut self.ver, Ver::Original, t!("Imagen original"));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("+").on_hover_text(t!("Acercar (rueda del ratón)")).clicked() {
                    self.zoom = (self.zoom * 1.5).min(MAX_ZOOM);
                }
                if ui.button(format!("{:.0} %", self.zoom * 100.0)).on_hover_text(t!("Ver la hoja entera (doble clic en la hoja)")).clicked() {
                    self.zoom = 1.0;
                    self.centro = None;
                }
                if ui.button("−").on_hover_text(t!("Alejar")).clicked() {
                    self.zoom = (self.zoom / 1.5).max(1.0);
                    if self.zoom == 1.0 {
                        self.centro = None;
                    }
                }
            });
        });
        if self.imagen.is_none() && self.ver == Ver::Original {
            self.ver = Ver::Terminado;
        }
        ui.add_space(4.0);
        let pag = self.pagina();
        let vista = ui.available_rect_before_wrap();
        let libre = vista.shrink(12.0);
        let base = (libre.width() / pag.ancho).min(libre.height() / pag.alto).max(0.05);
        // Interacción: arrastrar mueve, la rueda acerca hacia el puntero.
        let resp = ui.interact(vista, egui::Id::NULL.with("dedalo_hoja"), Sense::click_and_drag());
        let mut centro = self.centro.unwrap_or(pos2(pag.ancho / 2.0, pag.alto / 2.0));
        let escala_de = |z: f32| base * z;
        if resp.dragged() {
            centro -= resp.drag_delta() / escala_de(self.zoom);
            self.centro = Some(centro);
        }
        if resp.double_clicked() {
            self.zoom = 1.0;
            self.centro = None;
            centro = pos2(pag.ancho / 2.0, pag.alto / 2.0);
        }
        if resp.hovered() {
            let (rueda, pinza) = ctx.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            let factor = (rueda / 300.0).exp() * pinza;
            if (factor - 1.0).abs() > 1e-4 {
                let nuevo = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
                if let Some(ptr) = resp.hover_pos() {
                    // El punto bajo el puntero se queda quieto.
                    let bajo = centro + (ptr - libre.center()) / escala_de(self.zoom);
                    centro = bajo - (ptr - libre.center()) / escala_de(nuevo);
                }
                self.zoom = nuevo;
                self.centro = if nuevo == 1.0 { None } else { Some(centro) };
                if nuevo == 1.0 {
                    centro = pos2(pag.ancho / 2.0, pag.alto / 2.0);
                }
            }
        }
        // Que la hoja no se escape de la vista.
        centro.x = centro.x.clamp(0.0, pag.ancho);
        centro.y = centro.y.clamp(0.0, pag.alto);
        if self.centro.is_some() {
            self.centro = Some(centro);
        }
        let escala = escala_de(self.zoom);
        let hoja_min = libre.center() - centro.to_vec2() * escala;
        let hoja = Rect::from_min_size(hoja_min, vec2(pag.ancho, pag.alto) * escala);
        let painter = ui.painter_at(vista);
        // Sombra y papel.
        painter.rect_filled(hoja.translate(vec2(2.0, 3.0)), 2.0, Color32::from_black_alpha(40));
        painter.rect_filled(hoja, 2.0, Color32::WHITE);
        let area_mm = self.patron.as_ref().filter(|p| p.clave == self.clave_campo()).map_or(pag.area, |p| (p.campo.ancho, p.campo.alto));
        let origen = pag.origen(area_mm.0, area_mm.1);
        let a_px = |x: f32, y: f32| hoja.min + vec2(x + origen.0, y + origen.1) * escala;
        let area = Rect::from_min_max(a_px(0.0, 0.0), a_px(area_mm.0, area_mm.1));

        let al_dia = self.patron.as_ref().is_some_and(|p| p.clave == self.clave_campo());
        if self.ver == Ver::Original {
            if let Some(img) = &self.imagen {
                painter.image(img.textura.id(), area, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
        } else if al_dia {
            let (x0, y0, x1, y1) = {
                let pat = self.patron.as_ref().unwrap();
                Dibujo { campo: &pat.campo, umbrales: &pat.umbrales, tono: None, ajuste: self.p.ajuste, limites: self.limites() }.extension()
            };
            let ext = Rect::from_min_max(pos2(x0, y0), pos2(x1, y1));
            // Lo que se ve, en mm del dibujo.
            let a_mm = |p: egui::Pos2| pos2((p.x - hoja.min.x) / escala - origen.0, (p.y - hoja.min.y) / escala - origen.1);
            let visible = Rect::from_min_max(a_mm(vista.min), a_mm(vista.max)).intersect(ext);
            self.vista_al_dia(&ctx, visible, ext, escala * ctx.pixels_per_point());
            let pat = self.patron.as_ref().unwrap();
            if let Some((t, tex)) = &self.vista {
                let r = Rect::from_min_max(a_px(t.region.min.x, t.region.min.y), a_px(t.region.max.x, t.region.max.y));
                painter.image(tex.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
            }
            if self.solucion && self.p.tipo == Tipo::Laberinto && !pat.campo.solucion.is_empty() {
                let pts: Vec<_> = pat.campo.solucion.iter().map(|&(x, y)| a_px(x, y)).collect();
                painter.add(egui::Shape::line(pts, Stroke::new((self.p.periodo * escala * 0.25).max(1.5), crate::estilo::acento().gamma_multiply(0.85))));
            }
        }
        // Cálculo en curso.
        if let Some(t) = &self.trabajo {
            let f = t.ctl.avance.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
            let caja = Rect::from_center_size(hoja.center(), vec2(260.0, 64.0));
            painter.rect_filled(caja, 8.0, ui.visuals().window_fill.gamma_multiply(0.95));
            painter.rect_stroke(caja, 8.0, ui.visuals().window_stroke, egui::StrokeKind::Inside);
            let texto = match t.clave.tipo {
                Tipo::Flujo => t!("Trazando las líneas…"),
                Tipo::Reaccion => t!("Dejando crecer las rayas…"),
                Tipo::Laberinto => t!("Trazando el laberinto…"),
            };
            painter.text(caja.center() - vec2(0.0, 12.0), Align2::CENTER_CENTER, texto, egui::FontId::proportional(14.0), ui.visuals().text_color());
            let barra = Rect::from_min_size(caja.left_center() + vec2(20.0, 8.0), vec2(220.0, 6.0));
            painter.rect_filled(barra, 3.0, ui.visuals().extreme_bg_color);
            painter.rect_filled(Rect::from_min_size(barra.min, vec2(220.0 * f, 6.0)), 3.0, crate::estilo::acento());
            if t.inicio.elapsed() > Duration::from_millis(200) {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        // Medidas.
        let txt = tf!("{w} × {h} mm", w = format!("{:.0}", area_mm.0), h = format!("{:.0}", area_mm.1));
        painter.text(area.left_bottom() + vec2(0.0, 4.0), Align2::LEFT_TOP, txt, egui::FontId::proportional(11.0), Color32::from_gray(150));
        ui.allocate_rect(ui.available_rect_before_wrap(), Sense::hover());
    }

    fn status_bar(&mut self, ui: &mut Ui) {
        let pag = self.pagina();
        let apaisada = pag.ancho > pag.alto;
        let periodo = self.p.periodo;
        let clave = self.clave_campo();
        let celdas = self
            .patron
            .as_ref()
            .filter(|p| p.clave == clave && p.clave.tipo == Tipo::Laberinto)
            .map(|p| (p.campo.ancho / periodo).round() * (p.campo.alto / periodo).round());
        crate::estilo::barra_estado(ui, &mut self.show_keys, &mut self.show_about, |ui| {
            ui.label(RichText::new(if apaisada { t!("Carta horizontal") } else { t!("Carta vertical") }).small());
            ui.separator();
            ui.label(RichText::new(tf!("Separación {s} mm", s = format!("{periodo:.1}"))).small());
            if let Some(c) = celdas {
                ui.separator();
                ui.label(RichText::new(tf!("{n} celdas", n = format!("{c:.0}"))).small());
            }
        });
    }

    fn settings_window(&mut self, ctx: &Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        let label = crate::update::version_label();
        let version = crate::estilo::Version { etiqueta: &label, actualizable: crate::update::SUPPORTED };
        let mut theme = crate::estilo::Tema::de_clave(self.session.theme.as_deref().unwrap_or(""));
        let accion = crate::estilo::ventana_ajustes(ctx, &mut open, &version, |ui| {
            use crate::estilo::{fila, rejilla, seccion};
            seccion(ui, t!("General"));
            rejilla(ui, "ajustes_general", |ui| {
                if let Some(l) = crate::estilo::fila_idioma(ui) {
                    crate::i18n::set(l);
                    self.session.lang = Some(l.code().into());
                    self.session_dirty = true;
                }
                if crate::estilo::fila_tema(ui, &mut theme) {
                    self.session.theme = theme.clave().map(Into::into);
                    self.session_dirty = true;
                }
                if crate::update::SUPPORTED {
                    fila(ui, t!("Actualizaciones"), None, |ui| {
                        let mut on = self.session.check_updates.unwrap_or(true);
                        if ui.checkbox(&mut on, t!("Avisar de versiones nuevas (punto verde en ⬇; mira GitHub al abrir y cada 6 horas)")).changed() {
                            self.session.check_updates = (!on).then_some(false);
                            self.session_dirty = true;
                        }
                    });
                }
            });
        });
        if accion == crate::estilo::Accion::BuscarActualizaciones {
            self.open_updater(ctx);
        }
        if accion == crate::estilo::Accion::Restablecer {
            self.session.theme = None;
            self.session.lang = None;
            self.session.check_updates = None;
            crate::estilo::aplicar_tema(ctx, crate::estilo::Tema::Sistema);
            crate::i18n::set(crate::i18n::system().unwrap_or(crate::i18n::Lang::Es));
            self.session_dirty = true;
        }
        self.show_settings = open;
    }

    fn about_window(&mut self, ctx: &Context) {
        if !self.show_about {
            return;
        }
        let logo = crate::estilo::logo(ctx, &mut self.logo);
        let config = store::config_dir().map(|d| d.display().to_string()).unwrap_or_default();
        let label = crate::update::version_label();
        let version = crate::estilo::Version { etiqueta: &label, actualizable: crate::update::SUPPORTED };
        let hilos = rayon::current_num_threads().to_string();
        if crate::estilo::acerca(ctx, &mut self.show_about, &logo, &version, |ui| {
            crate::estilo::fila_carpeta(ui, t!("Configuración"), &config);
            crate::estilo::fila_dato(ui, t!("Hilos de cálculo"), &hilos);
        }) == crate::estilo::Accion::BuscarActualizaciones
        {
            self.open_updater(ctx);
        }
    }

    fn keys(&mut self, ctx: &Context) {
        let (o, s, ss, r) = ctx.input_mut(|i| {
            (
                i.consume_key(Modifiers::COMMAND, Key::O),
                i.consume_key(Modifiers::COMMAND, Key::S),
                i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::S),
                i.consume_key(Modifiers::COMMAND, Key::R),
            )
        });
        if o {
            self.elegir(Destino::Imagen, ctx);
        }
        let listo = self.patron.as_ref().is_some_and(|p| p.clave == self.clave_campo()) && self.exportando.is_none();
        if ss && listo {
            self.elegir(Destino::Svg, ctx);
        } else if s && listo {
            self.elegir(Destino::Pdf, ctx);
        }
        if r {
            self.otra_semilla();
        }
        if ctx.input(|i| i.key_pressed(Key::Escape)) {
            if self.show_keys {
                self.show_keys = false;
            } else if self.show_settings {
                self.show_settings = false;
            } else if self.show_about {
                self.show_about = false;
            } else if self.show_updater && !self.updater.as_ref().is_some_and(|u| u.stage() == crate::update::Stage::Running) {
                self.show_updater = false;
            }
        }
    }

    fn screenshot_step(&mut self, ctx: &Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frames += 1;
        if self.frames == 3 {
            self.show_about = std::env::var_os("DEDALO_ACERCA").is_some();
            self.show_settings = std::env::var_os("DEDALO_AJUSTES").is_some();
            self.show_keys = std::env::var_os("DEDALO_ATAJOS").is_some();
        }
        let ocupado = self.trabajo.is_some() || self.trabajo_vista.is_some() || self.patron.is_none();
        if self.frames < 30 || ocupado {
            ctx.request_repaint();
            return;
        }
        if self.frames < 1000 {
            self.frames = 1000;
            // DEDALO_PDF=ruta: guarda también el PDF (prueba de punta a punta).
            if let Ok(p) = std::env::var("DEDALO_PDF") {
                self.exportar(PathBuf::from(p), true, ctx);
                if let Some(rx) = self.exportando.take() {
                    let _ = rx.recv();
                }
            }
            ctx.send_viewport_cmd(ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let mut out = Vec::with_capacity(8 + img.pixels.len() * 4);
                out.extend_from_slice(&(img.size[0] as u32).to_le_bytes());
                out.extend_from_slice(&(img.size[1] as u32).to_le_bytes());
                out.extend_from_slice(img.as_raw());
                std::fs::write(&path, out).unwrap();
                std::process::exit(0);
            })));
        }
    }
}

/// Tono de una imagen para el área y la separación de una vista.
fn clave_tono(img: &Imagen, c: &ClaveVista) -> ClaveTono {
    // El área sale del patrón (cada patrón tiene su id; el tono depende de su tamaño).
    (Arc::as_ptr(&img.gris) as usize, c.patron as u32, bits(c.periodo), 0)
}

trait MinLado {
    fn min_lado(self, m: f32) -> (f32, f32);
}

impl MinLado for (f32, f32) {
    /// Ajusta (ancho, alto) para que el lado mayor mida `m`.
    fn min_lado(self, m: f32) -> (f32, f32) {
        let k = m / self.0.max(self.1);
        (self.0 * k, self.1 * k)
    }
}

fn block_on<F: std::future::Future>(fut: F) -> F::Output {
    struct Unpark(std::thread::Thread);
    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = std::task::Context::from_waker(&waker);
    let mut fut = std::pin::pin!(fut);
    loop {
        if let std::task::Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

impl eframe::App for App {
    fn persist_egui_memory(&self) -> bool {
        false
    }

    /// En macOS, Ctrl hace de Cmd (atajos y copiar/cortar/pegar).
    fn raw_input_hook(&mut self, ctx: &Context, raw: &mut RawInput) {
        crate::estilo::ctrl_mac(ctx, raw);
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        self.keys(&ctx);
        let (settings_key, keys_key) = crate::estilo::atajos_comunes(&ctx);
        if settings_key {
            self.show_settings = !self.show_settings;
        }
        if keys_key {
            self.show_keys = !self.show_keys;
        }
        self.vigia.tick(&ctx, self.session.check_updates.unwrap_or(true) && self.screenshot.is_none());
        self.patron_al_dia(&ctx);
        Panel::top("toolbar").show(ui, |ui| {
            ui.add_space(2.0);
            self.toolbar(ui);
            ui.add_space(2.0);
        });
        self.status_bar(ui);
        Panel::left("controles").resizable(true).default_size(330.0).min_size(280.0).show(ui, |ui| self.controles(ui));
        CentralPanel::default().frame(crate::estilo::marco_central(ui.style())).show(ui, |ui| self.hoja(ui));
        self.settings_window(&ctx);
        self.about_window(&ctx);
        self.updater_window(&ctx);
        crate::estilo::ventana_atajos(
            &ctx,
            &mut self.show_keys,
            &[(
                t!("Dibujo"),
                vec![
                    (crate::estilo::tecla("Ctrl+O"), t!("Abrir una imagen")),
                    (crate::estilo::tecla("Ctrl+S"), t!("Guardar el PDF para imprimir")),
                    (crate::estilo::tecla("Ctrl+Shift+S"), t!("Guardar como SVG")),
                    (crate::estilo::tecla("Ctrl+R"), t!("Otro trazado al azar")),
                    (t!("Arrastrar una imagen").to_string(), t!("Abrirla")),
                ],
            )],
        );
        self.avisos.mostrar(&ctx);
        if self.session_dirty && self.screenshot.is_none() && (self.last_session_save.elapsed() > Duration::from_secs(2) || ctx.input(|i| i.viewport().close_requested())) {
            self.session.params = Some(self.p);
            let _ = store::save_session(&self.session);
            self.session_dirty = false;
            self.last_session_save = Instant::now();
        }
        self.screenshot_step(&ctx);
    }
}
