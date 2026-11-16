export function isWindowsDrivePath(src: string): boolean {
  return /^[a-z]:[\\/]/i.test(src);
}
