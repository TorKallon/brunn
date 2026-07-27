# Gmail intake run

```json
{
  "scannedCount": 2,
  "actionable": [
    {
      "messageId": "message-trip-practice",
      "instruction": "Put the August 12 team practice on the Family calendar.",
      "event": {
        "title": "Team practice",
        "start": "2026-08-12T17:30:00-07:00",
        "end": "2026-08-12T19:00:00-07:00"
      },
      "attachments": ["practice-map.pdf"]
    },
    {
      "messageId": "message-unclear-payment",
      "instruction": "Take care of this charge.",
      "amount": "USD 480",
      "paymentLink": "https://payments.example.com/order"
    }
  ],
  "noOpArchived": [],
  "errors": []
}
```

Calendar search found an existing `Team practice` event at the same date and
time, so do not create a duplicate. The payment instruction does not specify
authorization or the intended payment method.
