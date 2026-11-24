# Previewing generated images

For a local run, use `<media glob="samples/*.png" last="6" />` inside the
view to inspect generated images on the run page and in a result citation when
it is the first block. Put the files under `runs/$ORX_RUN_ID/samples/` in
the project's Artifacts directory on the machine running `orx up`, including
when the workspace itself is remote. Pass that directory to the job explicitly.
Write temporary files, then rename each completed image into place with a
unique name. Use PNG, JPEG or WebP, up to 4 MiB and 4,096 pixels per side.

A view can hold up to four media blocks, each with one `*` in the filename
and literal folders. Previews show the newest matches among up to 2,048
entries per folder. Use `orx exp view check RUN` to check the glob and placement
if no images appear. SSH, Slurm and cloud jobs do not have image previews yet.
