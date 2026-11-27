//! Retratos de ejemplo incluidos en el programa (dominio público, Wikimedia
//! Commons), reducidos a 720 px: para probar sin buscar una imagen.

pub struct Ejemplo {
    /// Se guarda en la sesión y da nombre al PDF/SVG.
    pub clave: &'static str,
    pub nombre: &'static str,
    pub datos: &'static [u8],
}

pub const EJEMPLOS: &[Ejemplo] = &[
    Ejemplo { clave: "lincoln", nombre: "Abraham Lincoln", datos: include_bytes!("../ejemplos/lincoln.jpg") },
    Ejemplo { clave: "einstein", nombre: "Albert Einstein", datos: include_bytes!("../ejemplos/einstein.jpg") },
    Ejemplo { clave: "douglass", nombre: "Frederick Douglass", datos: include_bytes!("../ejemplos/douglass.jpg") },
    Ejemplo { clave: "curie", nombre: "Marie Curie", datos: include_bytes!("../ejemplos/curie.jpg") },
    Ejemplo { clave: "mona-lisa", nombre: "Mona Lisa", datos: include_bytes!("../ejemplos/mona-lisa.jpg") },
];

pub fn buscar(clave: &str) -> Option<&'static Ejemplo> {
    EJEMPLOS.iter().find(|e| e.clave == clave)
}

#[cfg(test)]
mod tests {
    #[test]
    fn todos_se_abren() {
        for e in super::EJEMPLOS {
            let i = image::load_from_memory(e.datos).unwrap();
            assert!(i.width() > 300 && i.height() > 300, "{}", e.clave);
        }
    }
}
