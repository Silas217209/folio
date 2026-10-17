Copy the `templates/` and `lib/` directories into your **library root**. Opening a
library never installs these files automatically.

`lib/kontakte.typ` is the phonebook. Add a key with an invoice-pro party dictionary
(`name`, `address`, `city`, `country`, optional `email`, `phone`, etc.). The
`absender` entry is the sender. Bank details are under `bankverbindungen` in the
same file. The supplied details are examples.

The library's folder menu offers **Contacts…** when `lib/kontakte.typ` exists.
It opens the phonebook in Folio's source editor with the preview hidden.

Each document starts with an import and one show statement:

```typst
#import "/lib/rechnung.typ": *
#show: rechnung.with(
  kunde: "freundlich", nummer: "42", datum: "2026-10-17", faellig: "2026-10-31",
)

Guten Tag,

vielen Dank für Ihren Auftrag. Wir berechnen die folgenden Leistungen:

#line-items[
  #item([Gestaltung], quantity: 2, price: 85)
]

Bitte überweisen Sie den Gesamtbetrag von *#info.total.gross* bis zum #info.due-date.

#bank-details(..konto("standard"), qr-code: (size: 25mm))

Mit freundlichen Grüßen,

#unterschrift
```

Select another key with `kunde: "atelier"`. To override a field in just this
document, use `kunde: kontakt("freundlich", address: "Neuer Weg 2")`. Both the
sender and recipient accept either a key or a party dictionary. Unknown keys
produce a Typst diagnostic listing available contacts.

The template is a starting document copied once when creating an invoice.
Greeting, introduction, payment wording, offer terms, and closing live directly
in each document. Edit them freely, add paragraphs or headings, and move or
remove sections before or after the table.

`lib/rechnung.typ` holds layout defaults and the `rechnung` / `angebot` show rules.
These set metadata and wrap your body in invoice-pro's `invoice`; they add no
letter text or payment blocks. `#info.due-date` reads the date from the show rule,
so changing `faellig` updates the date in your text. `#info.total.gross` inserts the calculated gross total, including currency
formatting; `#info.total.net` inserts the net total. They update when the items
change. See the [documented info module](https://leonieziechmann.github.io/invoice-pro/api-reference/components#info-module).

The visible `bank-details` call generates the QR code with the calculated amount
due and invoice reference. Change its account key or options in an individual
invoice. Offers omit that call. `#unterschrift` inserts the shared signature image
(if configured); the closing above it is ordinary editable text. You can also
use native `payment-goal` or `signature` components in place of the plain wording.

Other native invoice options can be passed through the show rule, such as
`service-period`, `payment-reference`, `tax`, and `notes`. Standard `item`,
`bundle`, `group`, `discount`, `surcharge`, and `prepayment` remain available.

The Rechnung template includes the editable service-date note
`Leistungsdatum gleich Rechnungsdatum`. Change it when the service date differs
from the invoice date. The following `block(breakable: false)` keeps the payment
text, bank details, and signature together, moving them to the next page when
needed. Everything inside remains editable Typst. Its payment sentence uses `#info.total.gross`; you can
include `#info.due-date` if you want a deadline in that sentence.

New documents are stored in `documents/Rechnung/` or `documents/Angebot/`.
PDF exports are grouped the same way under `pdf/`. The `lib/` directory stays out
of the invoice list. Put custom layouts and images in `lib/` and use root-relative
imports such as `/lib/layout.typ`.

Each PDF export snapshots shared files as well as the document, so older
versions retain their original contacts and setup after the shared files change.

API reference: https://leonieziechmann.github.io/invoice-pro/api-reference/components
