//! La hoja (tamaño carta) y la salida a PDF o SVG.

use serde::{Deserialize, Serialize};
use std::io::Write;

/// Carta (US Letter) en mm.
pub const CARTA: (f32, f32) = (215.9, 279.4);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tamano {
    /// Toda la hoja (menos el margen); horizontal si la imagen es apaisada.
    #[serde(rename = "hoja")]
    Hoja,
    /// Un cuadro centrado de `cuadro_cm` de lado mayor.
    #[serde(rename = "cuadro")]
    Cuadro,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Impresion {
    /// Contorno negro y el muro en gris claro: se ve qué rellenar.
    #[serde(rename = "guia")]
    Guia,
    /// Solo el contorno.
    #[serde(rename = "contorno")]
    Contorno,
    /// Muros rellenos: el resultado terminado.
    #[serde(rename = "relleno")]
    Relleno,
}

/// Dónde va el dibujo en la hoja.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pagina {
    pub ancho: f32,
    pub alto: f32,
    /// Lo que puede medir el área de dibujo (ya con las proporciones de la imagen).
    pub area: (f32, f32),
}

impl Pagina {
    /// `aspecto` = ancho / alto de la imagen.
    pub fn new(tamano: Tamano, cuadro_cm: f32, margen: f32, aspecto: f32) -> Pagina {
        let apaisada = tamano == Tamano::Hoja && aspecto > 1.0;
        let (ancho, alto) = if apaisada { (CARTA.1, CARTA.0) } else { CARTA };
        let (mut bw, mut bh) = (ancho - 2.0 * margen, alto - 2.0 * margen);
        if tamano == Tamano::Cuadro {
            let lado = cuadro_cm * 10.0;
            bw = bw.min(lado);
            bh = bh.min(lado);
        }
        let (bw, bh) = (bw.max(10.0), bh.max(10.0));
        let area = if bw / bh > aspecto { (bh * aspecto, bh) } else { (bw, bw / aspecto) };
        Pagina { ancho, alto, area }
    }

    /// Esquina superior izquierda de un dibujo de `w` × `h` mm centrado.
    pub fn origen(&self, w: f32, h: f32) -> (f32, f32) {
        ((self.ancho - w) / 2.0, (self.alto - h) / 2.0)
    }
}

/// Estilo del trazo.
#[derive(Clone, Copy, Debug)]
pub struct Trazo {
    pub impresion: Impresion,
    /// Grosor de la línea del contorno (mm).
    pub linea: f32,
    /// Gris del muro en modo guía (0 = negro, 1 = blanco).
    pub gris: f32,
}

/// PDF de una página con los contornos (mm, origen en la esquina del dibujo).
pub fn pdf(pag: &Pagina, origen: (f32, f32), figuras: &[Vec<(f32, f32)>], trazo: Trazo, titulo: &str) -> Vec<u8> {
    const PT: f32 = 72.0 / 25.4;
    let mut c = String::new();
    // Unidades en mm, y hacia abajo, origen en la esquina del dibujo.
    c += &format!("q {PT:.5} 0 0 {:.5} {:.3} {:.3} cm\n", -PT, origen.0 * PT, (pag.alto - origen.1) * PT);
    c += &format!("1 j 1 J {:.3} w 0 G {:.3} g\n", trazo.linea, match trazo.impresion {
        Impresion::Relleno => 0.0,
        _ => trazo.gris,
    });
    for f in figuras {
        for (k, (x, y)) in f.iter().enumerate() {
            c += &format!("{x:.2} {y:.2} {}\n", if k == 0 { "m" } else { "l" });
        }
        c += "h\n";
    }
    c += match trazo.impresion {
        Impresion::Guia => "B*\n",
        Impresion::Contorno => "S\n",
        Impresion::Relleno => "f*\n",
    };
    c += "Q\n";
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(c.as_bytes()).unwrap();
    let datos = z.finish().unwrap();

    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut xref = vec![];
    let mut obj = |out: &mut Vec<u8>, cuerpo: &[u8]| {
        xref.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", xref.len()).as_bytes());
        out.extend_from_slice(cuerpo);
        out.extend_from_slice(b"\nendobj\n");
    };
    obj(&mut out, b"<< /Type /Catalog /Pages 2 0 R >>");
    obj(&mut out, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    let pagina = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {:.2} {:.2}] /Contents 4 0 R /Resources << >> >>", pag.ancho * PT, pag.alto * PT);
    obj(&mut out, pagina.as_bytes());
    let mut flujo = format!("<< /Length {} /Filter /FlateDecode >>\nstream\n", datos.len()).into_bytes();
    flujo.extend_from_slice(&datos);
    flujo.extend_from_slice(b"\nendstream");
    obj(&mut out, &flujo);
    let info = format!("<< /Title ({}) /Creator (dedalo) >>", escapar_pdf(titulo));
    obj(&mut out, info.as_bytes());
    let inicio = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", xref.len() + 1).as_bytes());
    for x in &xref {
        out.extend_from_slice(format!("{x:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R /Info 5 0 R >>\nstartxref\n{inicio}\n%%EOF\n", xref.len() + 1).as_bytes());
    out
}

/// Solo ASCII imprimible (el título es informativo).
fn escapar_pdf(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii() && !c.is_ascii_control()).map(|c| if matches!(c, '(' | ')' | '\\') { format!("\\{c}") } else { c.to_string() }).collect()
}

/// SVG del tamaño de la hoja.
pub fn svg(pag: &Pagina, origen: (f32, f32), figuras: &[Vec<(f32, f32)>], trazo: Trazo) -> String {
    let mut d = String::new();
    for f in figuras {
        for (k, (x, y)) in f.iter().enumerate() {
            d += &format!("{}{:.2} {:.2}", if k == 0 { "M" } else { "L" }, x + origen.0, y + origen.1);
        }
        d += "Z";
    }
    let gris = (trazo.gris * 255.0).round() as u8;
    let estilo = match trazo.impresion {
        Impresion::Guia => format!("fill=\"#{gris:02x}{gris:02x}{gris:02x}\" stroke=\"#000\" stroke-width=\"{:.3}\"", trazo.linea),
        Impresion::Contorno => format!("fill=\"none\" stroke=\"#000\" stroke-width=\"{:.3}\"", trazo.linea),
        Impresion::Relleno => "fill=\"#000\"".to_string(),
    };
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}mm\" height=\"{h}mm\" viewBox=\"0 0 {w} {h}\">\n<rect width=\"{w}\" height=\"{h}\" fill=\"#fff\"/>\n<path fill-rule=\"evenodd\" stroke-linejoin=\"round\" {estilo} d=\"{d}\"/>\n</svg>\n",
        w = pag.ancho,
        h = pag.alto
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pagina_carta() {
        let p = Pagina::new(Tamano::Hoja, 15.0, 15.0, 0.5);
        assert_eq!((p.ancho, p.alto), CARTA);
        assert!((p.area.1 - (CARTA.1 - 30.0)).abs() < 0.01);
        let h = Pagina::new(Tamano::Hoja, 15.0, 15.0, 2.0);
        assert_eq!((h.ancho, h.alto), (CARTA.1, CARTA.0), "apaisada");
        let c = Pagina::new(Tamano::Cuadro, 12.0, 15.0, 1.0);
        assert_eq!(c.area, (120.0, 120.0));
    }

    #[test]
    fn pdf_valido() {
        let p = Pagina::new(Tamano::Cuadro, 10.0, 10.0, 1.0);
        let f = vec![vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0)]];
        let b = pdf(&p, p.origen(100.0, 100.0), &f, Trazo { impresion: Impresion::Guia, linea: 0.2, gris: 0.9 }, "prueba");
        assert!(b.starts_with(b"%PDF-1.4") && b.ends_with(b"%%EOF\n"));
        // La tabla xref apunta al principio de cada objeto.
        let cola = String::from_utf8_lossy(&b[b.len() - 400..]).into_owned();
        let inicio: usize = cola.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(b[inicio..].starts_with(b"xref"));
        let tabla = String::from_utf8_lossy(&b[inicio..]).into_owned();
        for (k, l) in tabla.lines().skip(3).take(5).enumerate() {
            let off: usize = l[..10].parse().unwrap();
            assert!(b[off..].starts_with(format!("{} 0 obj", k + 1).as_bytes()), "objeto {}", k + 1);
        }
    }
}
