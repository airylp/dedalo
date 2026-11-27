# Modelos de rostro

De MediaPipe (Google), licencia Apache-2.0 (https://github.com/google-ai-edge/mediapipe/blob/master/LICENSE),
sin modificar, descargados de https://storage.googleapis.com/mediapipe-assets/:

- `face_detection_full_range.tflite`: detector de caras BlazeFace de rango completo (entrada 192×192).
- `face_landmark.tflite`: malla de 468 puntos del rostro (entrada 192×192).

Los ejecuta `src/rostro.rs` con tract (Rust puro).
