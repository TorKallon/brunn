# Northwind weekend arrangement evidence

Booking, payment, allocation, resource availability, and actual use are
independent states. Allocation records whether an arrangement has secured a
place or resource; it does not claim that the underlying resource is generally
available.

- `arrangement:lodging-801` is confirmed and deposit paid, with an allocated
  room held, but actual use is `not_started`.
- `arrangement:rental-804` is requested with allocation `waitlisted`; payment
  is `not_due` and resource availability remains `unknown`.
- `arrangement:ticket-803` is issued and paid with an allocated seat, but
  actual use remains `not_started` because no boarding evidence exists.
- `arrangement:registration-802` may become `used` only from attendance
  evidence. Payment or confirmation alone never proves use.

Reservation, registration, ticket, transport, lodging, and rental are
arrangement profiles; handoff uses the same state model in its own evidence.
