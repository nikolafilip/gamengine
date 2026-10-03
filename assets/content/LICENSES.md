# Licences of the content's files

Every file under `assets/content/` that is not written by this project is listed here with
where it came from and under what terms (CONTENT.md 2 and 11: CC0 and ours only). The
official content as a whole is CC-BY-SA 4.0 (LICENSE.md at the root).

## Models (`models/`)

| File | From | Licence |
|---|---|---|
| `props/sword.glb` | KayKit Adventurers Character Pack 1.0 (`sword_1handed.gltf` + `knight_texture.png`), Kay Lousberg, https://github.com/KayKit-Game-Assets/KayKit-Character-Pack-Adventures-1.0 | CC0 1.0 |
| `props/dagger.glb` | the same pack (`dagger.gltf` + `rogue_texture.png`) | CC0 1.0 |
| `props/staff.glb` | the same pack (`staff.gltf` + `mage_texture.png`) | CC0 1.0 |
| `props/crossbow.glb` | the same pack (`crossbow_1handed.gltf` + `rogue_texture.png`) | CC0 1.0 |
| `props/hammer.glb` | ours: `gm-tools content synth hammer` | CC-BY-SA 4.0 |
| `props/musket.glb` | ours: `gm-tools content synth musket` | CC-BY-SA 4.0 |

The pack's files were packed into single `.glb`s by `gm-tools content import prop`
(the glTF JSON, its `.bin` and its texture, unchanged otherwise).

## Fonts (`ui/`)

| File | From | Licence |
|---|---|---|
| `ui/fira_sans_medium.ttf` | Fira Sans Medium, The Mozilla Foundation and Telefonica S.A., via github.com/google/fonts (`ofl/firasans/FiraSans-Medium.ttf`) | SIL OFL 1.1 (`ui/OFL-fira_sans.txt`) |
| `ui/medievalsharp.ttf` | MedievalSharp, Wojciech Kalinowski, via github.com/google/fonts (`ofl/medievalsharp`) | SIL OFL 1.1 (`ui/OFL-medievalsharp.txt`) |

## The skin (`ui/*.png`) and icons (`icons/*.png`)

Drawn for this project (`scripts/dev/skin-gen.py` writes the skin, each piece once per
density: `name.png`, `name@2x.png`, `name@3x.png`, `name@4x.png`), CC-BY-SA 4.0 unless a
row above says otherwise.
