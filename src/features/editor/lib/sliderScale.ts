/** Keep neutral at the same position without changing a parameter's real limits. */
export function sliderPosition(value: number, min: number, max: number): number {
  if (!(min < 0 && max > 0) || min === -max) return value;
  const fraction = value < 0 ? (value - min) / -min / 2 : .5 + value / max / 2;
  return min + fraction * (max - min);
}

export function sliderParameter(position: number, min: number, max: number): number {
  if (!(min < 0 && max > 0) || min === -max) return position;
  const fraction = (position - min) / (max - min);
  return fraction < .5 ? min * (1 - fraction * 2) : max * (fraction * 2 - 1);
}
