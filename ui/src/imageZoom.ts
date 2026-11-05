export function imageWheelZoom(zoom: number, deltaY: number, deltaMode: number, viewportHeight: number): number {
  const pixels = deltaY * (deltaMode === 1 ? 16 : deltaMode === 2 ? viewportHeight : 1);
  return Math.max(0.25, Math.min(4, zoom * Math.exp(-pixels * 0.002)));
}
