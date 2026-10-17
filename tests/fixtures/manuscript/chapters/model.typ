= A simple model

We describe the system using a state variable $x(t)$
and a positive rate $lambda$. The evolution follows
the ordinary differential equation

$ (dif x)/(dif t) = -lambda x(t), quad x(0) = x_0. $

Its solution is the exponential response

$ x(t) = x_0 e^(-lambda t). $

The rate controls how quickly the initial state
settles. A smaller value preserves the influence of
the past; a larger value brings the system toward
equilibrium more rapidly.

#v(2mm)
#table(
  columns: (1fr, 1fr, 1fr),
  inset: 7pt,
  stroke: 0.4pt + luma(210),
  table.header([*Rate*], [*Time*], [*Response*]),
  [$lambda = 0.5$], [$t = 1$], [$0.607 x_0$],
  [$lambda = 1.0$], [$t = 1$], [$0.368 x_0$],
  [$lambda = 2.0$], [$t = 1$], [$0.135 x_0$],
)

= Discussion hello

A good model reveals its assumptions. Here, a single
parameter is enough to explain the essential motion.
The next step is to ask what changes when the system
is coupled to its surroundings.
