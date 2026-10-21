# Diagrama de arquitectura

[`docs/arquitectura.html`](../../docs/arquitectura.html) es el diagrama interactivo de la
arquitectura de Freedom y [`docs/arquitectura.json`](../../docs/arquitectura.json) su
especificación. Se generan con
[asimetrix-arq-generator](https://github.com/tt-a1i/archify) 1.0.0 (el skill de Claude Code),
pero con la identidad de Freedom: la marca del lienzo es el pájaro y el marco abierto de
`Icon::Brand` con la palabra «Freedom», y la paleta es la de la interfaz (tinta y ámbar).

## Regenerarlo

1. Aplica la identidad de Freedom a una copia del generador (no toca la instalación
   original; necesita `fontTools` y la fuente Montserrat Regular, SIL OFL 1.1):

   ```sh
   python tools/arquitectura/rebrand_arq.py <ruta del plugin asimetrix-arq-generator> <copia> <Montserrat-Regular.ttf>
   ```

2. Edita `docs/arquitectura.json`. Si los componentes citan archivos (`sources`), fija en
   `meta.repository.revision` un commit que ya esté en GitHub.
3. Valida y entrega desde la copia:

   ```sh
   node <copia>/bin/arq.mjs validate architecture docs/arquitectura.json --quality showcase --json --repo-root .
   node <copia>/bin/arq.mjs deliver architecture docs/arquitectura.json docs/arquitectura.html --quality showcase --json --repo-root .
   node <copia>/bin/arq.mjs visual-check docs/arquitectura.html --json
   ```

   `visual-check` deja capturas `docs/arquitectura.visual-check.*` junto al HTML: no se
   versionan, salvo las dos imágenes del README.
4. Las imágenes del README (`docs/arquitectura-dark.png` y `docs/arquitectura-light.png`)
   son el lienzo recortado de las capturas de 2048×1320 de `visual-check`.

El chequeo `check-asimetrix-brand` del generador falla en la copia a propósito: verifica el
logo de Asimetrix, que aquí se reemplaza.
