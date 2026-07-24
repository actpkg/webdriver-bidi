# ACT-HTTP wraps tool args in `arguments` and results in `content[].data`.
#
# navigate succeeds, and the events the mock interleaved before each response
# are buffered rather than mistaken for the command response itself.
POST {{baseurl}}/tools/navigate
{ "arguments": { "url": "https://example.com" } }
HTTP 200
[Asserts]
jsonpath "$.content[0].data.url" == "https://example.com"
jsonpath "$.content[0].data.navigation" == "nav-1"

POST {{baseurl}}/tools/console_drain
{ "arguments": {} }
HTTP 200
[Asserts]
jsonpath "$.content[0].data.entries" count > 0
jsonpath "$.content[0].data.dropped" == 0
jsonpath "$.content[0].data.entries[0].method" == "log.entryAdded"

# Second drain is empty — the buffer was consumed by the first.
POST {{baseurl}}/tools/console_drain
{ "arguments": {} }
HTTP 200
[Asserts]
jsonpath "$.content[0].data.entries" count == 0
