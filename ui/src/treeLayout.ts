export interface TreeLayoutNode {
  id: string;
  width: number;
  height: number;
  children: TreeLayoutNode[];
}

interface Branch {
  node: TreeLayoutNode;
  children: { branch: Branch; offset: number }[];
  left: number[];
  right: number[];
}

/** Pack sibling contours instead of reserving the widest part of each subtree
 * at every depth. Rows use actual card heights so descriptions cannot overlap. */
export function compactTreeLayout(roots: TreeLayoutNode[], gapX = 28, gapY = 48): Map<string, { x: number; y: number }> {
  function pack(branches: Branch[]) {
    const left: number[] = [];
    const right: number[] = [];
    const children = branches.map((branch) => {
      let offset = 0;
      if (right.length) {
        offset = -Infinity;
        for (let depth = 0; depth < Math.min(right.length, branch.left.length); depth++) {
          offset = Math.max(offset, right[depth] + gapX - branch.left[depth]);
        }
      }
      for (let depth = 0; depth < branch.left.length; depth++) {
        left[depth] = Math.min(left[depth] ?? Infinity, branch.left[depth] + offset);
        right[depth] = Math.max(right[depth] ?? -Infinity, branch.right[depth] + offset);
      }
      return { branch, offset };
    });
    return { children, left, right };
  }

  function layout(node: TreeLayoutNode): Branch {
    const packed = pack(node.children.map(layout));
    const center = packed.children.length
      ? (packed.children[0].offset + packed.children[packed.children.length - 1].offset) / 2
      : 0;
    return {
      node,
      children: packed.children.map((child) => ({ ...child, offset: child.offset - center })),
      left: [-node.width / 2, ...packed.left.map((x) => x - center)],
      right: [node.width / 2, ...packed.right.map((x) => x - center)],
    };
  }

  const forest = pack(roots.map(layout));
  const rowHeights: number[] = [];
  function measure(branch: Branch, depth: number) {
    rowHeights[depth] = Math.max(rowHeights[depth] ?? 0, branch.node.height);
    branch.children.forEach((child) => measure(child.branch, depth + 1));
  }
  forest.children.forEach((child) => measure(child.branch, 0));
  const rowY = [0];
  for (let depth = 1; depth < rowHeights.length; depth++) {
    rowY[depth] = rowY[depth - 1] + rowHeights[depth - 1] + gapY;
  }
  const origin = forest.left.reduce((minimum, x) => Math.min(minimum, x), 0);
  const positions = new Map<string, { x: number; y: number }>();
  function place(branch: Branch, center: number, depth: number) {
    positions.set(branch.node.id, { x: center - branch.node.width / 2 - origin, y: rowY[depth] });
    branch.children.forEach((child) => place(child.branch, center + child.offset, depth + 1));
  }
  forest.children.forEach((child) => place(child.branch, child.offset, 0));
  return positions;
}
