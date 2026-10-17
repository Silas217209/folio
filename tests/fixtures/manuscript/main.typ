#set page(
  paper: "a4",
  margin: (x: 24mm, y: 22mm),
  numbering: "1",
)
#set text(font: "Libertinus Serif", size: 10.5pt)
#set par(justify: true, leading: 0.7em)
#set heading(numbering: "1.1")
#show heading.where(level: 1): it => {
  v(0.6em)
  text(size: 15pt, weight: "bold", it)
  v(0.3em)
}

#align(center)[
  #v(3mm)
  #text(size: 25pt, weight: "bold")[
    A note on quiet systems
  ]
  #v(3mm)
  #text(size: 12pt)[Notes on continuity and change]
  #v(5mm)
  #text(size: 10pt, fill: gray)[
    Alex Morgan · Research notes · October 2026
  ]
  #v(7mm)
]

#block(inset: (x: 8mm, y: 3mm))[
  *Abstract.* A useful system makes room for the work.
  In this note, we consider how small, local changes
  accumulate into coherent behavior, and why the
  simplest model is often the most revealing.
]

#include "chapters/introduction.typ"
#include "chapters/model.typ"