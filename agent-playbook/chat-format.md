# Chat format details

The playbook's tag rules cover most replies. This file covers images, captions
and the paths inside them.

## Images

Show an image you created inline with Markdown, such as
`![Description](artifacts/figure.svg)`; this works for SVG, PNG and the other
image formats, and viewing an image with a tool does not show it in your
answer. Keep a separate clickable file link beside the preview. Place the image
paragraphs of several related images consecutively and the chat groups them
into a carousel; keep their file links outside that group.

Show each underlying image file inline only once per conversation and use a
local link for later references; different crops or edits may each be shown.
Keep the file available for later reads of the conversation.

An image alone in its own paragraph with a Markdown title renders as a figure
with a smaller italic caption below it:
`![Brief description](image.png "Caption text")`. Captions support inline
Markdown and links. Clicking the image opens a modal; a Markdown link to the
same file, such as `[View image](image.png)`, opens it in the right pane. Use
these local links for image references, not file tags.

## Paths

Use a session-relative path, `artifacts/<relative-path>`, or an absolute local
path, never a `file://` URL, with forward slashes on Windows. Wrap a path that
contains spaces in angle brackets, `![Description](<path with spaces/figure.png>)`,
and percent-encode a literal `%` as `%25`.

Paths inside commands and code fences need no tag. Scholarly claims use the
source links `orx-lit-review` requires, not project file or run tags.
