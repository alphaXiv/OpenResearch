# Before you hand it over

- **Read the audit line `save()` prints.** It checks the printed width, the
  Type 42 font embedding, stray axes titles, missing axis labels, text below
  the 5pt floor, text that overlaps other text, and text running off the
  canvas. `clean` is the bar; anything else is a defect to fix, not a warning
  to note and move past.
- One audit finding is not yours to fix: if it reports no publication font on
  the machine, say so when you hand the figure over rather than installing
  fonts, which is a change to the environment you should ask about first.
- A label placed at a reference line or a data point is the usual source of an
  overlap: it lands on a tick label at some data ranges and not others. Move
  it, or give it `backgroundcolor="white"` so it masks what it covers.
- Open the PDF and read it at printed size. If a tick label is unreadable on
  screen at 100%, it is unreadable on paper.
- Every axis labelled, with units. No stray `ax.set_title`.
- The claim in the caption is visible in the figure without the prose.
- Uncertainty shown, with n stated.
- No unexplained series, no legend entry for something that was cut.
- The script reruns from scratch and reproduces the same file.
