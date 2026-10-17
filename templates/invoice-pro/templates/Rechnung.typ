#import "/lib/rechnung.typ": *
#show: rechnung.with(
  kunde: "freundlich", // or kontakt("freundlich", address: "Eine andere Adresse")
  nummer: "{{number}}",
  datum: "{{date}}",
  faellig: "{{due}}",
  titel: "{{title}}",
  art: "{{type}}",
)

Sehr geehrte Damen und Herren,

hiermit berechnen wir

#line-items(show-column: (pos: true, unit: false, quantity: true, unit-price: true), show-information: false)[
  #item([Arbeitsstunden], quantity: 1, price: 30.50)
  #item([Grafikarbeit], quantity: 1, price: 127.57)
  #item([Arbeitsstunden], quantity: 1, price: 30.50)
  #item([Druck], quantity: 1, price: 110.27)
]

#set par(spacing: 1em)
#text(size: 9pt)[Leistungsdatum gleich Rechnungsdatum]

#block(breakable: false, above: 1em, below: 0pt)[
  Bitte überweisen Sie den Rechnungsbetrag in Höhe von #info.total.gross auf folgendes Konto. Vielen Dank!

  #bank-details(..konto("standard"), qr-code: (size: 25mm))

  Mit freundlichen Grüßen

  #v(8mm)
  #unterschrift
]
