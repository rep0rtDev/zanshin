-- Core events v4: the rest of the standard catalog (immutable schemas) and currency rates.
INSERT INTO event_definitions (name, description, value_type, is_core) VALUES
    ('adImpression',          'Ad shown; value = placement; metadata.network, metadata.amount + currency for ad revenue', 'any',    true),
    ('currencyGranted',       'Soft/hard currency granted; value = source; metadata.currency, metadata.amount',          'any',    true),
    ('currencySpent',         'Soft/hard currency spent; value = sink; metadata.currency, metadata.amount',              'any',    true),
    ('itemGranted',           'Inventory item granted; value = item id; metadata.quantity',                              'any',    true),
    ('itemSpent',             'Inventory item traded/spent; value = item id; metadata.quantity',                         'any',    true),
    ('itemConsumed',          'Inventory item consumed; value = item id; metadata.quantity',                             'any',    true),
    ('eventLeaderboardScore', 'Score submitted to an event leaderboard; value = leaderboard id; metadata.score',         'any',    true),
    ('eventLeaderboardRank',  'Final rank in an event leaderboard; value = leaderboard id; metadata.rank, metadata.tier','any',    true),
    ('achievementUnlocked',   'Achievement unlocked; value = achievement id',                                            'any',    true),
    ('questStarted',          'Quest started; value = quest id',                                                         'any',    true),
    ('questCompleted',        'Quest completed; value = quest id',                                                       'any',    true),
    ('inviteSent',            'Invite sent; value = channel',                                                            'any',    true),
    ('inviteAccepted',        'Invite accepted; value = inviter external id',                                            'any',    true),
    ('socialShare',           'Content shared; value = channel',                                                         'any',    true),
    ('appUpdated',            'Client updated; value = new version',                                                     'string', true)
ON CONFLICT (name) DO UPDATE SET is_core = true, discovered = false;

CREATE TABLE IF NOT EXISTS currency_rates (
    day          date NOT NULL,
    code         text NOT NULL,
    usd_per_unit double precision NOT NULL,
    source       text NOT NULL DEFAULT 'manual',
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (day, code)
);
