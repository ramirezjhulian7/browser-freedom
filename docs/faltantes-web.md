# Faltantes vistos en sitios reales

Lo que falla al navegar con Freedom por sitios reales, para priorizar el trabajo de la fase 2.
Cada fila dice qué se vio, dónde, y si quedó verificado con una página de prueba aislada
(`freedom session` sobre `master` en `08ff44e`, 2026-10-21) o si la causa todavía hay que
investigarla.

## Funciones que faltan (verificado)

| Falta | Síntoma en los sitios | Spec | Dónde va |
|---|---|---|---|
| Constructor `Image` (`new Image()`) | `ReferenceError: Image is not defined` al precargar imágenes; el script se corta. Visto en un sitio de videos. | HTML §4.8.3 (legacy factory function de `HTMLImageElement`) | 2.2, `bf-web` html |
| `indexedDB` | `ReferenceError: indexedDB is not defined` en una promesa al arrancar. Visto en un sitio de videos. | Indexed Database API 3.0 | 2.2, `bf-web` bom (storage) |
| `innerWidth`/`innerHeight` y `documentElement.clientWidth` fijos en 800×600 | En una ventana de 1180×704 los scripts ven 800×600: los diseños que se adaptan con JavaScript calculan mal. | CSSOM View §7, §6 | 2.2, `bf-web` cssom (el viewport real de la pestaña) |
| `IntersectionObserver` | `undefined`: carga diferida de imágenes y listas infinitas no arrancan. | Intersection Observer | 2.2 |
| `ResizeObserver` | `undefined`. | Resize Observer | 2.2 |
| `customElements` | `undefined`: los sitios con componentes web no se definen. | HTML §4.13 | 2.2 |
| `Worker` | `undefined`. | HTML §10 | 2.3 |
| `crypto.subtle` | `undefined` (sí existe `crypto`). | Web Cryptography API | 2.2 |
| `localStorage` en páginas `file:` | `SecurityError: The operation is insecure`. Chrome y Firefox lo permiten (origen opaco con almacenamiento propio por archivo o carpeta). | HTML §12.2 | 2.2, `bf-web` bom |

Ya funcionan (verificado en la misma prueba): eventos de clic al DOM, `fetch`,
`XMLHttpRequest`, `querySelectorAll`, `classList`, `innerHTML`, `document.currentScript`,
`location`, `history.pushState`, `MutationObserver` y `WebSocket` (al menos como interfaz).

## Errores en sitios por investigar

| Sitio | Lo que se ve | Pista |
|---|---|---|
| Google (búsquedas) | Responde `429 Too Many Requests` y redirige a `/sorry` con reCAPTCHA; el reCAPTCHA falla con `TypeError: Type error.` dentro de `recaptcha__es_419.js`. Buscar desde la barra queda inutilizable. | Google es el buscador por defecto: es lo más visible para quien usa Freedom. Ver si el 429 depende del `User-Agent` nuevo, de las cabeceras (`Sec-CH-UA`, `Accept`) o del ritmo de peticiones, y qué operación falla en el reCAPTCHA. |
| Aplicación Next.js (asimetrix.co) | `TypeError: Cannot read properties of undefined (reading 'dplId')` al registrar los chunks de Turbopack. | Algún global o propiedad que el runtime de Next.js espera y no existe. |
| Aplicación Vite (emmavet.com.co) | `TypeError: Type error.` en la línea 1 del módulo principal: la aplicación no arranca. | Falla al evaluar un módulo ES; el mensaje no dice qué operación. |
| Sitio de videos | `TypeError: Cannot read properties of undefined (reading 'protocol')` en su script de cabecera. | Algo devuelve `undefined` donde el sitio espera un objeto con `protocol` (¿`<a>.protocol`, `URL` de un recurso, `location` de un iframe?). |
| Analítica de Ahrefs | `TypeError: Failed to construct 'URL': Invalid URL 'undefined'`. | `new URL(undefined)` falla bien por spec: el problema es lo que el script lee antes (probablemente `document.currentScript` en scripts insertados por otro script, o su `src`). |

## Mensajes de error

`bf-js` lanza `TypeError: Type error.` sin decir la operación ni la propiedad (reCAPTCHA y la
aplicación Vite). Con mensajes como los de los demás motores (`x is not a function`,
`Cannot read properties of undefined (reading 'y')`) estos casos se diagnostican desde la
consola, sin reproducirlos a mano.

## Pruebas que fallan en `master`

- `bf-web`, `tests/cssom.rs`, `snapshot_from_the_shell_is_used`: `devicePixelRatio` da 1 donde
  el test espera 2 (desde el merge de CSSOM).
