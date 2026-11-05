import assert from "node:assert/strict";
import test from "node:test";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { Button, ButtonLink } from "../src/components/ui/Button.tsx";
import { IconButton, IconButtonLink } from "../src/components/ui/IconButton.tsx";
import { MenuItem } from "../src/components/ui/MenuItem.tsx";
import { Switch } from "../src/components/ui/Switch.tsx";

test("standard controls suppress native titles, including untyped spreads and ancestor titles", () => {
  for (const Control of [Button, ButtonLink, IconButton, IconButtonLink, MenuItem, Switch]) {
    const html = renderToStaticMarkup(React.createElement("div", { title: "ancestor" },
      React.createElement(Control, { title: "leaked", "aria-label": "Action", "data-tip": "Custom hint" })));
    assert.match(html, /title=""/);
    assert.doesNotMatch(html, /title="leaked"/);
    assert.match(html, /aria-label="Action"/);
    assert.match(html, /data-tip="Custom hint"/);
  }
});
