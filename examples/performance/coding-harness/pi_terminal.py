"""Final generation evidence from the pinned Pi JSON session lifecycle."""


def pi_outcome(events):
    settlements = []
    observed_stops = []
    started = False
    pending = False
    terminal = None
    for event in events:
        kind = event.get('type')
        if kind == 'agent_start':
            started = True
            pending = True
            terminal = None
        elif kind in ('message_start', 'message_update', 'message_end'):
            message = event.get('message', {})
            role = message.get('role') if isinstance(message, dict) else None
            if kind == 'message_end' and role == 'assistant':
                observed_stops.append(message.get('stopReason'))
            # JSON message_update omits message and only carries assistant deltas.
            if kind == 'message_update' or role in ('user', 'assistant'):
                pending = True
                terminal = None
        elif kind == 'agent_end':
            terminal = None
            messages = event.get('messages')
            if started and event.get('willRetry') is False and isinstance(messages, list):
                assistant = next((message for message in reversed(messages)
                                  if isinstance(message, dict) and message.get('role') == 'assistant'), {})
                terminal = assistant.get('stopReason')
            started = False
            pending = True
        elif kind == 'agent_settled':
            settlements.append({'stop_reason': terminal, 'aborted': event.get('aborted')})
            started = False
            pending = False
            terminal = None
    return {'settlements': settlements, 'pending': pending, 'observed_stop_reasons': observed_stops}


def successful_pi_generations(outcome, expected_turns):
    """Clean final generations, not a guarantee of successful CLI teardown or complete billing."""
    settlements = outcome.get('settlements', [])
    return (type(expected_turns) is int and expected_turns > 0
            and outcome.get('pending') is False and len(settlements) == expected_turns
            and all(item.get('stop_reason') == 'stop' and item.get('aborted') is False
                    for item in settlements))
