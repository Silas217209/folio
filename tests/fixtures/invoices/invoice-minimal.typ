// Edit this document directly. All invoice details live in Typst.
// Edit the values below, then adjust the layout further down if you like.
#let details = (
  number: "{{number}}",
  title: "{{type}}",
  subject: "{{title}}",
  issued: "{{date}}",
  due: "{{due}}",
  currency: "EUR",
  seller: (
    name: "Your business",
    address: "Street address\nPostal code, city",
    email: "hello@example.com",
    registration: "",
    payment: "Bank name\nIBAN / account number\nPayment reference",
  ),
  customer: "Customer name",
  address: "Street address\nPostal code, city",
  items: (
    (description: "Description of your service", quantity: 1, unit-price: 100.00),
  ),
  tax-rate: 0, // Percentage. Change this to the rate for your invoice.
  note: "Thank you for your business.",
)

#let invoice(data) = {
  let accent = rgb("30383b")
  set page(paper: "a4", margin: (x: 22mm, y: 22mm), footer: context {
    set text(size: 8pt, fill: rgb("888888"))
    [#data.seller.name #h(1fr) #counter(page).display("1")]
  })
  set text(font: "Libertinus Sans", size: 10pt, fill: rgb("30383b"))
  set par(leading: 0.65em)
  let lines(value) = value.split("\n").join(linebreak())
  let money(cents) = {
    let whole = str(calc.floor(cents / 100))
    let fraction = str(calc.rem(cents, 100))
    whole + "." + (if fraction.len() == 1 { "0" } else { "" }) + fraction + " " + data.currency
  }
  let amounts = data.items.map(item => int(calc.round(item.quantity * item.unit-price * 100)))
  let subtotal = amounts.sum()
  let tax = int(calc.round(subtotal * data.tax-rate / 100))
  grid(columns: (1fr, auto), column-gutter: 20mm,
    [#text(size: 19pt, weight: "regular", fill: accent, data.seller.name)
     #v(3mm)
     #lines(data.seller.address)
     #if data.seller.email != "" { [#linebreak()#data.seller.email] }],
    align(right)[#text(size: 24pt, weight: "regular", data.title)
      #v(3mm)
      #text(fill: accent, weight: "bold", data.number)],
  )
  v(15mm)
  grid(columns: (1fr, 1fr), column-gutter: 20mm,
    [#text(size: 8pt, fill: rgb("888888"), "BILL TO")
     #v(3mm)
     #strong(data.customer)
     #linebreak()#lines(data.address)],
    align(right)[Issued: #data.issued #linebreak() Due: #data.due],
  )
  v(10mm)
  text(size: 13pt, weight: "bold", data.subject)
  v(5mm)
  table(
    columns: (1fr, 15mm, 33mm, 33mm),
    align: (left, right, right, right),
    inset: (x: 3mm, y: 4mm),
    stroke: none,
    fill: (x,y) => if y == 0 { rgb("f6f6f4") } else { none },
    table.header([*Description*], [*Qty*], [*Unit price*], [*Amount*]),
    table.hline(stroke: 0.5pt + accent.lighten(50%)),
    ..data.items.enumerate().map(((i,item)) => (
      item.description,
      str(item.quantity),
      money(int(calc.round(item.unit-price * 100))),
      money(amounts.at(i)),
    )).flatten(),
  )
  v(4mm)
  align(right, block(width: 75mm)[
    #grid(columns: (1fr,auto), align: (left, right), column-gutter: 4mm, row-gutter: 3mm,
      [Subtotal], money(subtotal),
      [Tax (#(data.tax-rate)%)], money(tax),
    )
    #v(3mm)
    #line(length: 100%, stroke: 0.7pt + accent)
    #v(2mm)
    #grid(columns: (1fr,auto), align: (left, right), column-gutter: 4mm, [#strong("Total")], [#text(size: 14pt, weight: "bold", fill: accent, money(subtotal + tax))])
  ])
  v(12mm)
  if data.seller.payment != "" {
    text(size: 8pt, fill: rgb("888888"), "PAYMENT DETAILS")
    v(3mm)
    lines(data.seller.payment)
    v(6mm)
  }
  if data.note != "" { lines(data.note); v(6mm) }
  if data.seller.registration != "" { text(size: 8pt, fill: rgb("777777"), lines(data.seller.registration)) }
}

#invoice(details)
