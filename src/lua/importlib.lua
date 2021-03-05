function strip_raw_outmeta()
   for k,v in pairs(outmeta) do
      if k:sub(1,4) == "raw_" then outmeta[k] = nil end
   end
end

function consume_tag(wat)
   local ret = inmeta[wat]
   inmeta[wat] = nil
   if ret ~= nil and ret ~= "" then
      ret = ret:gsub("^[ \t]+",""):gsub("[ \t]+$","")
   end
   if ret == "" then return nil
   else return ret
   end
end

function set_raw_outmeta()
   for k,v in pairs(inmeta) do
      outmeta["raw_"..k] = v
   end
end

-- (three-character tags are ID3v2, four-character tags are ID3v2.3 or ID3v2.4)
local ID3V2_TAGS<const> = {
   BUF = "recommended_buffer_size",
   CNT = "play_counter",
   COM = "comment",
   CRA = "audio_encryption",
   CRM = "encrypted_meta_frame",
   ETC = "event_timing_codes",
   EQU = "equalization",
   GEO = "general_encapsulated_object",
   IPL = "involved_people_list",
   LNK = "linked_information",
   MCI = "music_cd_identifier",
   MLL = "mpeg_location_lookup_table",
   PIC = "attached_picture",
   POP = "popularimeter",
   REV = "reverb",
   RVA = "relative_volume_adjustment",
   SLT = "synchronized_lyric_or_text",
   STC = "synced_tempo_codes",
   TAL = "album",
   TBP = "bpm",
   TCM = "composer",
   TCO = "content_type",
   TCR = "copyright_message",
   TDA = "date",
   TDY = "playlist_delay",
   TEN = "encoded_by",
   TFT = "file_type",
   TIM = "time",
   TKE = "initial_key",
   TLA = "language",
   TLE = "length",
   TMT = "media_type",
   TOA = "original_artist_or_performer",
   TOF = "original_filename",
   TOL = "original_lyricist_or_text_writer",
   TOR = "original_release_year",
   TOT = "original_album",
   TP1 = "artist",
   TP2 = "orchestra",
   TP3 = "conductor",
   TP4 = "interpreter",
   TPA = "part_of_a_set",
   TPB = "publisher",
   TRC = "international_standard_recording_code",
   TRD = "recording_dates",
   TRK = "track",
   TSI = "size",
   TSS = "encoder",
   TT1 = "grouping",
   TT2 = "title",
   TT3 = "subtitle",
   TXT = "lyricist_or_text_writer",
   TXX = "user_defined_text_information_frame",
   TYE = "year",
   UFI = "unique_file_identifier",
   ULT = "unsynchronized_lyric_or_text_transcription",
   WAF = "official_audio_file_webpage",
   WAR = "official_artist_webpage",
   WAS = "official_audio_source_webpage",
   WCM = "commercial_information",
   WCP = "copyright_or_legal_information",
   WPB = "publisher_official_webpage",
   WXX = "user_defined_url_link_frame",
   AENC = "audio_encryption",
   APIC = "attached_picture",
   ASPI = "audio_seek_point_index",
   COMM = "comment",
   COMR = "commercial_frame",
   ENCR = "encryption_method_registration",
   EQU2 = "equalization",
   ETCO = "event_timing_codes",
   GEOB = "general_encapsulated_object",
   GRID = "group_identification_registration",
   LINK = "linked_information",
   MCDI = "music_cd_identifier",
   MLLT = "mpeg_location_lookup_table",
   OWNE = "ownership_frame",
   PRIV = "private_frame",
   PCNT = "play_counter",
   POPM = "popularimeter",
   POSS = "position_synchronisation_frame",
   RBUF = "recommended_buffer_size",
   RVA2 = "relative_volume_adjustment",
   RVRB = "reverb",
   SEEK = "seek_frame",
   SIGN = "signature_frame",
   SYLT = "synchronised_lyric_or_text",
   SYTC = "synchronised_tempo_codes",
   TALB = "album",
   TBPM = "bpm",
   TCOM = "composer",
   TCON = "content_type",
   TCOP = "copyright_message",
   TDEN = "encoding_time",
   TDLY = "playlist_delay",
   TDOR = "original_releaseOtime",
   TDRC = "recording_time",
   TDRL = "release_time",
   TDTG = "tagging_time",
   TENC = "encoded_by",
   TEXT = "lyricist_or_text_writer",
   TFLT = "file_type",
   TIPL = "involved_people_list",
   TIT1 = "grouping",
   TIT2 = "title",
   TIT3 = "subtitle",
   TKEY = "initial_key",
   TLAN = "language",
   TLEN = "length",
   TMCL = "musician_credits_list",
   TMED = "media_type",
   TMOO = "mood",
   TOAL = "original_album",
   TOFN = "original_filename",
   TOLY = "original_lyricist_or_textwriter",
   TOPE = "original_artist",
   TOWN = "file_owner_or_licensee",
   TPE1 = "artist",
   TPE2 = "orchestra",
   TPE3 = "conductor",
   TPE4 = "interpreter",
   TPOS = "part_of_a_set",
   TPRO = "produced_notice",
   TPUB = "publisher",
   TRCK = "track",
   TRSN = "internet_radio_station_name",
   TRSO = "internet_radio_station_owner",
   TSOA = "sort_album",
   TSOP = "sort_artist",
   TSOT = "sort_title",
   TSRC = "international_standard_recording_code",
   TSSE = "software_or_hardware_and_settings_used_for_encoding",
   TSST = "set_subtitle",
   TXXX = "user_defined_text_information_frame",
   UFID = "unique_file_identifier",
   USER = "terms_of_use",
   USLT = "unsynchronised_lyric_or_text_transcription",
   WCOM = "commercial_information",
   WCOP = "copyright_or_legal_information",
   WOAF = "official_audio_file_webpage",
   WOAR = "official_artist_webpage",
   WOAS = "official_audio_source_webpage",
   WORS = "official_internet_radio_station_homepage",
   WPAY = "payment",
   WPUB = "publisher_official_webpage",
   WXXX = "user_defined_url_link_frame",
}
function remap_id3v2_tags()
   for tag, base in pairs(ID3V2_TAGS) do
      local value = consume_tag(tag)
      if value then
         local key = "id3v2_"..base
         if not inmeta[key] then
            inmeta[key] = value
         end
      end
   end
end
