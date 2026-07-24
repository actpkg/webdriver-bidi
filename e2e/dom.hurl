# get_text goes through script.evaluate and returns text/plain.
POST {{baseurl}}/tools/get_text
{ "arguments": {} }
HTTP 200
[Asserts]
jsonpath "$.content[0].data" == "mock text"
jsonpath "$.content[0].mime_type" == "text/plain"

# click is two-step: resolve the selector to a node handle via script.evaluate,
# then dispatch pointer actions against that element origin.
POST {{baseurl}}/tools/click
{ "arguments": { "selector": "#go" } }
HTTP 200
