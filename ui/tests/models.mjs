/** A catalog model as `/api/harnesses` sends it. The server's reading of its name defaults to one model per id; `parsed` overrides it. */
export const model = (id, displayName, parsed = {}) => ({
  id,
  ...(displayName && { displayName }),
  parsed: {
    key: id, family: id, series: id, version: null, label: displayName ?? id,
    variant: { effort: null, fast: false, thinking: false, context: null }, variantRank: 0, ...parsed,
  },
});
