// Shared show rules; import this file from every document.
// All invoice-pro components remain available through this import.
#import "@preview/invoice-pro:0.5.0": *
#import "/lib/kontakte.typ": kontakt, konto

#let voreinstellungen = (
  theme: themes.DIN-5008(
    font: "Latin Modern Sans", color-row-even: none,
    hole-mark: false, folding-marks: false,
  ),
  locale: locale.de-de,
  tax-exempt-small-biz: false,
)
#let unterschrift = none

#let iso-datum(value) = {
  if type(value) != str { return value }
  let parts = value.split("-").map(int)
  datetime(year: parts.at(0), month: parts.at(1), day: parts.at(2))
}

// Layout and metadata only. All wording and content live in the document.
// Extra native invoice options override the shared defaults.
#let rechnung(
  inhalt,
  kunde: "freundlich",
  absender: "absender",
  nummer: none,
  datum: datetime.today(),
  faellig: none,
  titel: none,
  art: "Rechnung",
  ..optionen,
) = {
  let empfaenger = if type(kunde) == str { kontakt(kunde) } else { kunde }
  let firma = if type(absender) == str { kontakt(absender) } else { absender }
  let frist = iso-datum(faellig)
  let config = voreinstellungen + (
    sender: firma, recipient: empfaenger,
    subject: art, invoice-nr: nummer, project: titel,
    date: iso-datum(datum), due-date: frist,
  ) + optionen.named()
  invoice(..config, inhalt)
}

#let angebot = rechnung.with(art: "Angebot", references: none)
