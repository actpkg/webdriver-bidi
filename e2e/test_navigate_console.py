async def test_navigate_then_drain_console(client, session_id, with_session):
    # navigate succeeds, and the events the mock interleaved before each
    # response are buffered rather than mistaken for the command response itself.
    result = await client.call_tool("navigate", with_session(session_id, url="https://example.com"))
    assert result.structured_content["url"] == "https://example.com"
    assert result.structured_content["navigation"] == "nav-1"

    result = await client.call_tool("console_drain", with_session(session_id))
    entries = result.structured_content["entries"]
    assert len(entries) > 0
    assert result.structured_content["dropped"] == 0
    assert entries[0]["method"] == "log.entryAdded"

    # Second drain is empty — the buffer was consumed by the first.
    result = await client.call_tool("console_drain", with_session(session_id))
    assert len(result.structured_content["entries"]) == 0
