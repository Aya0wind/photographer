import sony from "../assets/brands/sony.svg?raw";
import canon from "../assets/brands/canon.svg?raw";
import nikon from "../assets/brands/nikon.svg?raw";
import fujifilm from "../assets/brands/fujifilm.svg?raw";
import leica from "../assets/brands/leica.svg?raw";
import hasselblad from "../assets/brands/hasselblad.svg?raw";
import dji from "../assets/brands/dji.svg?raw";
import omsystem from "../assets/brands/omsystem.svg?raw";
import ricoh from "../assets/brands/ricoh.svg?raw";
import vivo from "../assets/brands/vivo.svg?raw";
import xiaomi from "../assets/brands/xiaomi.svg?raw";
import huawei from "../assets/brands/huawei.svg?raw";
import zeiss from "../assets/brands/zeiss.svg?raw";

// Original vector paths from the official websites; see docs/brand-frames.md.
export const OFFICIAL_BRAND_LOGOS: Record<string, {svg: string; white: boolean}> = {
  "SONY": {svg: sony, white: true},
  "Canon": {svg: canon, white: false},
  "Nikon": {svg: nikon, white: false},
  "FUJIFILM": {svg: fujifilm, white: false},
  "Leica": {svg: leica, white: false},
  "Hasselblad": {svg: hasselblad, white: true},
  "DJI": {svg: dji, white: true},
  "OM SYSTEM": {svg: omsystem, white: false},
  "RICOH": {svg: ricoh, white: false},
  "vivo": {svg: vivo, white: true},
  "Xiaomi": {svg: xiaomi, white: false},
  "HUAWEI": {svg: huawei, white: false},
  "ZEISS": {svg: zeiss, white: false},
};
export function officialLogoData(brand: string): {url: string; white: boolean} | null {
  const logo = OFFICIAL_BRAND_LOGOS[brand];
  if (!logo) return null;
  return { url: `data:image/svg+xml;base64,${btoa(Array.from(new TextEncoder().encode(logo.svg), b => String.fromCharCode(b)).join(""))}`, white: logo.white };
}
