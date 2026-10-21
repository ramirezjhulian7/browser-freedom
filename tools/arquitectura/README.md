# Diagramas de arquitectura

[`docs/arquitectura.html`](../../docs/arquitectura.html) (el recorrido de una página) y
[`docs/dependencias.html`](../../docs/dependencias.html) (las dependencias entre crates) son los
diagramas interactivos de Freedom, y los `.json` del mismo nombre, sus especificaciones. Se generan con
[asimetrix-arq-generator](https://github.com/tt-a1i/archify) 1.0.0 (el skill de Claude Code),
pero con la identidad de Freedom: la marca del lienzo es el pájaro y el marco abierto de
`Icon::Brand` con la palabra «Freedom», y la paleta es la de la interfaz (tinta y ámbar).

## Regenerarlo

1. Aplica la identidad de Freedom a una copia del generador (no toca la instalación
   original; necesita `fontTools` y la fuente Montserrat Regular, SIL OFL 1.1):

   ```sh
   python tools/arquitectura/rebrand_arq.py <ruta del plugin asimetrix-arq-generator> <copia> <Montserrat-Regular.ttf>
   ```

2. Edita la especificación (`docs/arquitectura.json` o `docs/dependencias.json`). Si los
   componentes citan archivos (`sources`), fija en `meta.repository.revision` un commit que ya
   esté en GitHub. Las flechas de `dependencias.json` son la reducción transitiva de los
   `[dependencies]` `bf-*` de cada `Cargo.toml`, sin `bf-infra` ni `bf-url`.
3. Valida y entrega desde la copia:

   ```sh
   node <copia>/bin/arq.mjs validate architecture docs/arquitectura.json --quality showcase --json --repo-root .
   node <copia>/bin/arq.mjs deliver architecture docs/arquitectura.json docs/arquitectura.html --quality showcase --json --repo-root .
   node <copia>/bin/arq.mjs visual-check docs/arquitectura.html --json
   # lo mismo con docs/dependencias.json y docs/dependencias.html
   ```

   `visual-check` deja capturas `docs/*.visual-check.*` junto al HTML: no se versionan.
4. Las imágenes del README (`docs/<diagrama>-dark.png` y `docs/<diagrama>-light.png`) son el
   lienzo recortado de las capturas de 2048×1320 de `visual-check`.

El chequeo `check-asimetrix-brand` del generador falla en la copia a propósito: verifica el
logo de Asimetrix, que aquí se reemplaza.
