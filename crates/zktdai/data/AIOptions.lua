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
}

return options
