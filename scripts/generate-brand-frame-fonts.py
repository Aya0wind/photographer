"""Freeze bundled variable fonts at the weights used by SVG preview/export.

Run manually with fonttools installed when updating the source TTFs. Generated
fonts are checked in; application builds never need Python or fonttools.
"""
from pathlib import Path

from fontTools.ttLib import TTFont
from fontTools.varLib.instancer import instantiateVariableFont


def main():
    root = Path(__file__).resolve().parents[1] / "src/features/editor/assets/fonts"
    for family in ("Inter", "Manrope"):
        for weight, style in ((400, "Regular"), (700, "Bold")):
            source = TTFont(root / f"{family}.ttf")
            axes = {
                axis.axisTag: weight if axis.axisTag == "wght" else axis.defaultValue
                for axis in source["fvar"].axes
            }
            font = instantiateVariableFont(source, axes, inplace=True)
            font["OS/2"].usWeightClass = weight
            if weight == 700:
                font["OS/2"].fsSelection = (font["OS/2"].fsSelection & ~64) | 32
                font["head"].macStyle |= 1
            else:
                font["OS/2"].fsSelection = (font["OS/2"].fsSelection & ~32) | 64
                font["head"].macStyle &= ~1
            names = {1: family, 16: family, 2: style, 17: style,
                     4: f"{family} {style}", 6: f"{family}-{style}"}
            for record in font["name"].names:
                if record.nameID in names:
                    record.string = names[record.nameID].encode(record.getEncoding())
            font.save(root / f"{family}-{style}.ttf")


if __name__ == "__main__":
    main()
