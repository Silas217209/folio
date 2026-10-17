// Keys stay the same when a contact's details change.
#let kontakte = (
  absender: (
    name: "Studio Muster", address: "Musterstraße 12", city: "12345 Musterstadt",
    country: "DE", tax-nr: "123/456/78901", email: "hallo@studio-muster.example",
  ),
  freundlich: (
    name: "Firma Freundlich", address: "Rechnungstraße 12", city: "45678 Ortsname", country: "DE",
  ),
  nord: (
    name: "Beispielbüro Nord", address: "Beispielweg 8", city: "23456 Beispielstadt", country: "DE",
  ),
  atelier: (
    name: "Atelier Beispiel", address: "Atelierstraße 5", city: "34567 Beispielstadt", country: "DE",
  ),
)

// Overrides affect this use only: kontakt("freundlich", address: "Neuer Weg 2").
#let kontakt(key, ..details) = {
  assert(key in kontakte, message: "Unbekannter Kontakt: " + key + ". Verfügbar: " + kontakte.keys().join(", "))
  kontakte.at(key) + details.named()
}

#let bankverbindungen = (
  standard: (name: "Studio Muster", bank: "Musterbank", iban: "DE07100202005821158846"),
)
#let konto(key) = {
  assert(key in bankverbindungen, message: "Unbekannte Bankverbindung: " + key)
  bankverbindungen.at(key)
}
