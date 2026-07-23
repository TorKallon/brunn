-- Candidate unused index drops from db_schema.tar collected 2026-04-25 21:16 PDT
-- Review FULLTEXT entries and run during a maintenance window. Excludes PRIMARY, UNIQUE, and detected FK-support indexes.

-- charlemagne.apscheduler_jobs.ix_apscheduler_jobs_next_run_time (BTREE) cols=(next_run_time) ops=0 confidence=high
ALTER TABLE `charlemagne`.`apscheduler_jobs` DROP INDEX `ix_apscheduler_jobs_next_run_time`;

-- charlemagne.apscheduler_jobs_old.ix_apscheduler_jobs_next_run_time (BTREE) cols=(next_run_time) ops=0 confidence=high
ALTER TABLE `charlemagne`.`apscheduler_jobs_old` DROP INDEX `ix_apscheduler_jobs_next_run_time`;

-- charlemagne.event_types.name (BTREE) cols=(name) ops=0 confidence=high
ALTER TABLE `charlemagne`.`event_types` DROP INDEX `name`;

-- charlemagne.event_types.shortname (BTREE) cols=(shortname) ops=0 confidence=high
ALTER TABLE `charlemagne`.`event_types` DROP INDEX `shortname`;

-- charlemagne.events.time (BTREE) cols=(start_time) ops=0 confidence=high
ALTER TABLE `charlemagne`.`events` DROP INDEX `time`;

-- charlemagne.user_bungie_profiles.globalNameFulltext (FULLTEXT) cols=(bungieGlobalDisplayName) ops=0 confidence=review-fulltext
ALTER TABLE `charlemagne`.`user_bungie_profiles` DROP INDEX `globalNameFulltext`;

-- charlemagne.user_bungie_profiles.membership_type (BTREE) cols=(membership_type, display_name) ops=0 confidence=high
ALTER TABLE `charlemagne`.`user_bungie_profiles` DROP INDEX `membership_type`;

-- discord.guild_members.nickname (FULLTEXT) cols=(nickname) ops=0 confidence=review-fulltext
ALTER TABLE `discord`.`guild_members` DROP INDEX `nickname`;

-- discord.users.username (FULLTEXT) cols=(username) ops=0 confidence=review-fulltext
ALTER TABLE `discord`.`users` DROP INDEX `username`;

-- nexus.carries_dungeons.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`carries_dungeons` DROP INDEX `membershipId`;

-- nexus.carries_raids.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`carries_raids` DROP INDEX `membershipId`;

-- nexus.clan_stat_types.id (BTREE) cols=(id) ops=0 confidence=high
ALTER TABLE `nexus`.`clan_stat_types` DROP INDEX `id`;

-- nexus.clan_stat_types.shortName (BTREE) cols=(shortName) ops=0 confidence=high
ALTER TABLE `nexus`.`clan_stat_types` DROP INDEX `shortName`;

-- nexus.clan_stats.clanStatTypeId (BTREE) cols=(clanStatTypeId, clanId, statValue) ops=0 confidence=high
ALTER TABLE `nexus`.`clan_stats` DROP INDEX `clanStatTypeId`;

-- nexus.dungeon_reparse.sum (BTREE) cols=(summarized) ops=0 confidence=high
ALTER TABLE `nexus`.`dungeon_reparse` DROP INDEX `sum`;

-- nexus.fireteams.membershipIdsIdx (FULLTEXT) cols=(membershipIds) ops=0 confidence=review-fulltext
ALTER TABLE `nexus`.`fireteams` DROP INDEX `membershipIdsIdx`;

-- nexus.fireteams_summary_pweaps.fireteamId (BTREE) cols=(fireteamId) ops=0 confidence=high
ALTER TABLE `nexus`.`fireteams_summary_pweaps` DROP INDEX `fireteamId`;

-- nexus.gboard_types.shortname (BTREE) cols=(shortName) ops=0 confidence=high
ALTER TABLE `nexus`.`gboard_types` DROP INDEX `shortname`;

-- nexus.nf_ls_reparse.sum (BTREE) cols=(summarized) ops=0 confidence=high
ALTER TABLE `nexus`.`nf_ls_reparse` DROP INDEX `sum`;

-- nexus.profiles.date_idx (BTREE) cols=(lastScanned) ops=0 confidence=high
ALTER TABLE `nexus`.`profiles` DROP INDEX `date_idx`;

-- nexus.profiles.displayName (BTREE) cols=(displayName) ops=0 confidence=high
ALTER TABLE `nexus`.`profiles` DROP INDEX `displayName`;

-- nexus.profiles.globalNameFulltext (FULLTEXT) cols=(bungieGlobalDisplayName) ops=0 confidence=review-fulltext
ALTER TABLE `nexus`.`profiles` DROP INDEX `globalNameFulltext`;

-- nexus.profiles.membershipType (BTREE) cols=(membershipType) ops=0 confidence=high
ALTER TABLE `nexus`.`profiles` DROP INDEX `membershipType`;

-- nexus.profiles.nameFulltext (FULLTEXT) cols=(displayName) ops=0 confidence=review-fulltext
ALTER TABLE `nexus`.`profiles` DROP INDEX `nameFulltext`;

-- nexus.profiles_sidecar.sum (BTREE) cols=(scanned) ops=0 confidence=high
ALTER TABLE `nexus`.`profiles_sidecar` DROP INDEX `sum`;

-- nexus.profiles_sidecar_trials.sum (BTREE) cols=(scanned) ops=0 confidence=high
ALTER TABLE `nexus`.`profiles_sidecar_trials` DROP INDEX `sum`;

-- nexus.raid_reparse.sum (BTREE) cols=(summarized) ops=0 confidence=high
ALTER TABLE `nexus`.`raid_reparse` DROP INDEX `sum`;

-- nexus.raw_dungeons.characterId (BTREE) cols=(characterId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_dungeons` DROP INDEX `characterId`;

-- nexus.raw_dungeons.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_dungeons` DROP INDEX `membershipId`;

-- nexus.raw_failed_pgcrs.lastFail (BTREE) cols=(lastFail) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_failed_pgcrs` DROP INDEX `lastFail`;

-- nexus.raw_menagerie.characterId (BTREE) cols=(characterId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_menagerie` DROP INDEX `characterId`;

-- nexus.raw_menagerie.instanceId (BTREE) cols=(instanceId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_menagerie` DROP INDEX `instanceId`;

-- nexus.raw_menagerie.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_menagerie` DROP INDEX `membershipId`;

-- nexus.raw_nightfalls.characterId (BTREE) cols=(characterId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls` DROP INDEX `characterId`;

-- nexus.raw_nightfalls.instanceId (BTREE) cols=(instanceId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls` DROP INDEX `instanceId`;

-- nexus.raw_nightfalls.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls` DROP INDEX `membershipId`;

-- nexus.raw_nightfalls_scored.characterId (BTREE) cols=(characterId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls_scored` DROP INDEX `characterId`;

-- nexus.raw_nightfalls_scored.instanceId (BTREE) cols=(instanceId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls_scored` DROP INDEX `instanceId`;

-- nexus.raw_nightfalls_scored.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_nightfalls_scored` DROP INDEX `membershipId`;

-- nexus.raw_raids.characterId (BTREE) cols=(characterId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_raids` DROP INDEX `characterId`;

-- nexus.raw_raids.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`raw_raids` DROP INDEX `membershipId`;

-- nexus.summary_crucible.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_crucible` DROP INDEX `membershipId`;

-- nexus.summary_crucible_old.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_crucible_old` DROP INDEX `membershipId`;

-- nexus.summary_gambit.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_gambit` DROP INDEX `membershipId`;

-- nexus.summary_gambit_prime.membershipId (BTREE) cols=(membershipId) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_gambit_prime` DROP INDEX `membershipId`;

-- nexus.summary_weapon_meta.kills (BTREE) cols=(kills) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_weapon_meta` DROP INDEX `kills`;

-- nexus.summary_weapon_meta_daily.kills (BTREE) cols=(kills) ops=0 confidence=high
ALTER TABLE `nexus`.`summary_weapon_meta_daily` DROP INDEX `kills`;

