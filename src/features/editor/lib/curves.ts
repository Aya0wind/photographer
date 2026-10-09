export type CurvePoint = [number, number];
export type CurveChannel = "rgb" | "red" | "green" | "blue" | "luminance";
export type CurvePicker = "point" | "black" | "gray" | "white";
export const IDENTITY: CurvePoint[] = [[0, 0], [255, 255]];
export const clampTone = (value: number) => Math.round(Math.max(0, Math.min(255, value)) * 100) / 100;

/** Natural cubic spline, matching PhotoCraft compose::curve_lut (clamped outside endpoints). */
export function curveSamples(points: CurvePoint[]): number[] {
  const n = points.length;
  const second = Array<number>(n).fill(0), c = Array<number>(n).fill(0), d = Array<number>(n).fill(0);
  for (let i = 1; i < n - 1; i++) {
    const left = points[i][0] - points[i - 1][0], right = points[i + 1][0] - points[i][0];
    const denominator = 2 * (left + right) - left * c[i - 1];
    c[i] = right / denominator;
    d[i] = (6 * ((points[i + 1][1] - points[i][1]) / right - (points[i][1] - points[i - 1][1]) / left) - left * d[i - 1]) / denominator;
  }
  for (let i = n - 2; i > 0; i--) second[i] = d[i] - c[i] * second[i + 1];
  return Array.from({ length: 256 }, (_, x) => {
    if (x <= points[0][0]) return points[0][1];
    if (x >= points[n - 1][0]) return points[n - 1][1];
    const i = points.findIndex((p) => p[0] >= x) - 1;
    const h = points[i + 1][0] - points[i][0];
    const a = (points[i + 1][0] - x) / h, b = (x - points[i][0]) / h;
    return clampTone(a * points[i][1] + b * points[i + 1][1] + ((a ** 3 - a) * second[i] + (b ** 3 - b) * second[i + 1]) * h * h / 6);
  });
}

export function addCurvePoint(points: CurvePoint[], input: number, output: number): { points: CurvePoint[]; index: number } {
  input = clampTone(input); output = clampTone(output);
  const near = points.findIndex(([x]) => Math.abs(x - input) < 1);
  if (near >= 0) return { points: points.map((p, i) => i === near ? [p[0], output] : p), index: near };
  if (points.length >= 19) return { points, index: -1 };
  const next: CurvePoint[] = [...points, [input, output]];
  next.sort((a, b) => a[0] - b[0]);
  return { points: next, index: next.findIndex(([x]) => x === input) };
}

export function moveCurvePoint(points: CurvePoint[], index: number, x: number, y: number): CurvePoint[] {
  const min = index === 0 ? 0 : points[index - 1][0] + 1;
  const max = index === points.length - 1 ? 255 : points[index + 1][0] - 1;
  return points.map((point, i) => i === index ? [clampTone(Math.max(min, Math.min(max, x))), clampTone(y)] : point);
}
