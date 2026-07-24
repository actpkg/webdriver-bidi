# Server 2 runs a session granted navigate + read + input, but NOT script.
POST {{baseurl2}}/tools/evaluate
{ "arguments": { "expression": "1+1" } }
HTTP 403
[Asserts]
jsonpath "$.error.kind" == "std:capability-denied"
jsonpath "$.error.message" contains "browser:script"

# click transitively needs browser:script to resolve the selector to a node
# handle, so it is denied even though browser:input IS granted. Asserting the
# message names browser:script is the point: it proves the coupling documented
# in the spec rather than just failing on the first missing capability.
POST {{baseurl2}}/tools/click
{ "arguments": { "selector": "#go" } }
HTTP 403
[Asserts]
jsonpath "$.error.kind" == "std:capability-denied"
jsonpath "$.error.message" contains "browser:script"

# Navigation is granted and still works.
POST {{baseurl2}}/tools/navigate
{ "arguments": { "url": "https://example.com" } }
HTTP 200
[Asserts]
jsonpath "$.content[0].data.url" == "https://example.com"
