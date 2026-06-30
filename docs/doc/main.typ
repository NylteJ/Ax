#import "@preview/hydra:0.6.3": hydra

#let title-fonts = ("Arial", "Source Han Sans")
#let body-fonts = ("Times New Roman", "STSong")

#set text(
  size: 13.33pt,
  font: body-fonts,
)

#set par(
  first-line-indent: (amount: 2em, all: true),
)

#set heading(numbering: "1.1.1.")

#show heading.where(
  level: 1,
): set text(
  size: 26pt,
  font: title-fonts,
)

#show heading.where(level: 1): it => pagebreak(weak: true) + it

#show heading.where(
  level: 2,
): set text(
  size: 18pt,
  font: title-fonts,
)

#show heading.where(
  level: 3,
): set text(
  size: 14pt,
  font: title-fonts,
)

#show footnote.entry: set text(size: 10pt)

#show raw: it => {
  show regex("\p{Han}"): set text(size: 1.06em, baseline: 0.03em)
  set text(font: ((name: "DejaVu Sans Mono", covers: "latin-in-cjk"), "KaiTi"))
  it
}

// ====== 内容正式开始 ======

#include "cover.typ"

#pagebreak()

#set page(paper: "a4", numbering: "I")
#outline(title: "目录")

#set page(
  header: context {
    counter(footnote).update(0)

    v(2.3em)
    place[#hydra(1, skip-starting: false) #h(1fr) #hydra(2, skip-starting: false)]
    place[#h(1fr) Ax 内核文档 #h(1fr)]
    line(start: (-2.5%, 0%), end: (102.5%, 0%))
  }
)

#set page(numbering: "1")
#counter(page).update(1)

#include "1.overview.typ"
#include "2.memory.typ"
#include "3.task.typ"
#include "4.filesystem.typ"
#include "5.hardware.typ"

#include "conclusion.typ"

// ==== 附录 ====

#set heading(numbering: "附录 A.")
#counter(heading).update(0)

#include "ref.typ"
#include "ai.typ"
