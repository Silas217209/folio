#import "/lib/rechnung.typ": *
#show: angebot.with(
  kunde: "freundlich", // or kontakt("freundlich", address: "Eine andere Adresse")
  nummer: "{{number}}",
  datum: "{{date}}",
  faellig: "{{due}}",
  titel: "{{title}}",
  art: "{{type}}",
)

Sehr geehrte Damen und Herren,

gerne bieten wir Ihnen die folgenden Leistungen an:

#line-items(show-column: (pos: true, unit: false, quantity: true, unit-price: true), show-information: false)[
  #item([Gestaltungskonzept], quantity: 1, price: 420.00)
  #item([Umsetzung], quantity: 1, price: 780.00)
]

Dieses Angebot ist bis zum #info.due-date gültig.
Die Umsetzung erfolgt nach schriftlicher Beauftragung und gemeinsamer Terminabsprache.

Mit freundlichen Grüßen,

#unterschrift
