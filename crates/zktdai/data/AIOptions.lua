--
--  Custom Options Definition Table format
--
--  These become the AI's start-script `[AIn] { [Options] }` values, readable
--  in Rust via `Callback::option(key)`; unset keys come in with these defaults.
--

local options = {
	{
		key    = 'territory_share',
		name   = 'Territory share',
		desc   = 'Percent of the map\'s metal spots to hold before expansion stops and the army turns fully offensive.',
		type   = 'number',
		def    = 50,
		min    = 10,
		max    = 90,
		step   = 5,
	},
	{
		key    = 'push_size',
		name   = 'Push army size',
		desc   = 'Combat units gathered before an all-in assault on the enemy base.',
		type   = 'number',
		def    = 12,
		min    = 4,
		max    = 60,
		step   = 1,
	},
	{
		key    = 'raiding',
		name   = 'Early raiding',
		desc   = 'Raider detachments box the enemy in and harass its perimeter from the opening, drawing its focus off our economy.',
		type   = 'bool',
		def    = true,
	},
	{
		key    = 'porc',
		name   = 'Perimeter porc',
		desc   = 'Ever-stronger turrets along the front line, with terraform ditches against vehicles.',
		type   = 'bool',
		def    = true,
	},
	{
		key    = 'sabotage',
		name   = 'Sabotage strikes',
		desc   = 'Surgical strikes (raiders, cloaked units, artillery) at spotted enemy critical targets that lack defenses.',
		type   = 'bool',
		def    = true,
	},
	{
		key    = 'porc_budget_pct',
		name   = 'Porc budget (% of income)',
		desc   = 'Per-window cap on defensive-turret spending, as a percent of metal income.',
		type   = 'number',
		def    = 20,
		min    = 5,
		max    = 100,
		step   = 5,
	},
	{
		key    = 'reserve_metal',
		name   = 'Metal reserve floor',
		desc   = 'Metal kept unspent at game start (~10% of starting capacity); the floor grows with storage afterwards.',
		type   = 'number',
		def    = 50,
		min    = 0,
		max    = 1000,
		step   = 10,
	},
	{
		key    = 'stockpile_metal',
		name   = 'Stockpile target',
		desc   = 'Metal stockpile to maintain (1500 + 300 headroom for stalls by default), spent reactively on counters/silos.',
		type   = 'number',
		def    = 1800,
		min    = 0,
		max    = 10000,
		step   = 100,
	},
	{
		key    = 'strategy',
		name   = 'Strategy profile',
		desc   = 'balanced | econ_greedy | porc_heavy; empty = picked by AI slot and logged.',
		type   = 'string',
		def    = '',
		maxlen = 24,
	},
}

return options
