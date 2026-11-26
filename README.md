# dedalo

Convierte una imagen en un **laberinto para imprimir y rellenar a mano**. Los muros engordan en las
zonas oscuras de la imagen y adelgazan en las claras: de cerca es un laberinto, y de lejos, una
vez rellenado con pluma, aparece la imagen.

![dedalo con líneas de flujo sobre la imagen de una esfera](docs/dedalo-flujo.png)

## Cómo funciona

1. **Abre una imagen** (PNG, JPEG, GIF, WebP o BMP), o arrástrala a la ventana. Brillo, contraste,
   niveles automáticos y negativo ajustan cómo se lee.
2. **Elige el patrón:**
   - **Líneas de flujo:** líneas que avanzan con curvas suaves sin cruzarse nunca. En lo oscuro se
     juntan y engordan; en lo claro se separan y adelgazan. Se colocan con el método de Jobard y
     Lefer (1997), con la separación según el tono. Su dirección mezcla ruido suave con la tangente
     de los bordes de la imagen («Siguen la imagen», de 0 a 100 %), así rodean las formas. Con
     **Zonas**, cada banda de tono se rellena con sus propias líneas en otra dirección. Las líneas
     nunca se acercan más que un canal mínimo y dos muros mínimos, así siempre cabe la pluma.
   - **Rayas (reacción-difusión):** el modelo de Gray-Scott con el ajuste «laberinto»
     (F = 0.029, k = 0.057). Son rayas que se ramifican, como una huella dactilar.
   - **Laberinto con salida:** un laberinto perfecto, con un solo camino de la entrada (arriba a la
     izquierda) a la salida (abajo a la derecha). La vista puede mostrar la solución, que no se
     imprime.
3. **Separación:** la distancia de un canal al siguiente. **Muro mínimo** y **canal mínimo**
   garantizan que siempre quede un trazo visible y sitio para la pluma.
4. **Hoja:** carta completa (horizontal si la imagen es apaisada) o un cuadro de N cm centrado, con
   margen.
5. **Guardar PDF** (o SVG), en vectores y a tamaño real. Se puede imprimir:
   - contorno y muro en gris: una guía de qué rellenar;
   - solo el contorno;
   - los muros ya rellenos, como se verá al terminar.

El tono decide, punto a punto, qué fracción de la superficie lleva tinta, igual que una trama de
imprenta. Por eso el patrón (lo lento de calcular) solo se rehace al cambiar su forma; brillo,
contraste y estilo de impresión se ven al momento.

![Laberinto «como se imprime»: contorno negro y muro gris](docs/dedalo-laberinto.png)

Detalle de los PDF (líneas de flujo a 200 ppp y laberinto a 300 ppp):

![Detalle del PDF con líneas de flujo](docs/dedalo-flujo-pdf.png)

![Detalle del PDF del laberinto](docs/dedalo-pdf-detalle.png)

Rayas de reacción-difusión:

![Rayas de reacción-difusión](docs/dedalo-rayas.png)

## Instalar

macOS y Linux (descarga la última versión publicada; vuelve a ejecutarlo para actualizar o
desinstalar):

```bash
curl -fsSL https://raw.githubusercontent.com/airylp/dedalo/main/install.sh | sh
```

Para compilarlo con lo último de `main`: `… | sh -s -- compilar` (instala Rust si falta).

Windows (PowerShell):

```powershell
[Net.ServicePointManager]::SecurityProtocol = 'Tls12'; irm https://raw.githubusercontent.com/airylp/dedalo/main/install.ps1 | iex
```

## Compilar

```sh
cargo build --release
cargo test --release
DEDALO_PNG=carpeta cargo test --release vista_previa -- --ignored --nocapture   # PNG y PDF de prueba
```

## Licencia

MIT.
