# Server 2 runs a session restricted to browser:navigate + browser:read, so
# script evaluation must be refused while navigation still works.
POST {{baseurl2}}/tools/evaluate
{ "arguments": { "expression": "1+1" } }
HTTP 403
[Asserts]
jsonpath "$.error.kind" == "std:capability-denied"
jsonpath "$.error.message" contains "browser:script"

# click transitively needs browser:script to resolve the selector, so denying
# script also denies clicking — the coupling documented in the spec.
POST {{baseurl2}}/tools/click
{ "arguments": { "selector": "#go" } }
HTTP 403
[Asserts]
jsonpath "$.error.kind" == "std:capability-denied"

# Navigation is granted and still works.
POST {{baseurl2}}/tools/navigate
{ "arguments": { "url": "https://example.com" } }
HTTP 200
[Asserts]
jsonpath "$.content[0].data.url" == "https://example.com"
