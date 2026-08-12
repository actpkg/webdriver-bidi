async def test_get_text_and_click(client, session_id, with_session):
    # get_text goes through script.evaluate and returns text/plain.
    result = await client.call_tool("get_text", with_session(session_id))
    assert result.content[0].text == "mock text"
    assert result.content[0].meta["dev.actcore/mime-type"] == "text/plain"

    # click is two-step: resolve the selector to a node handle via
    # script.evaluate, then dispatch pointer actions against that element
    # origin. The old hurl file asserted only HTTP 200 here (no jsonpath) —
    # preserved as "the call succeeds", the same claim in pytest terms.
    result = await client.call_tool("click", with_session(session_id, selector="#go"))
    assert not result.is_error
