import React from "react";
import { createRoot } from "react-dom/client";
import { QueryClientProvider } from "@tanstack/react-query";
import { Md, ChatImageScope } from "../../src/components/Md";
import { ArtifactMarkdown } from "../../src/components/ArtifactsTab";
import { createWorkspace, WorkspaceContext } from "../../src/workspace";

const workspace = createWorkspace("markdown-image-test");
const markdown = "![Remote](https://example.com/image.png)\n\n![Protocol relative](//example.com/other.png)\n\n![Local](local.svg)\n\nText between images.\n\n![](HTTPS://example.com/no-alt.png)";
createRoot(document.getElementById("root")!).render(<WorkspaceContext value={workspace}><QueryClientProvider client={workspace.queryClient}>
  <section id="chat"><ChatImageScope projectId="p"><Md text={markdown} /></ChatImageScope></section>
  <section id="artifact"><ArtifactMarkdown projectId="p" folder="" markdown={markdown} entries={[]} /></section>
</QueryClientProvider></WorkspaceContext>);
