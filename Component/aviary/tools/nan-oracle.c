// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Macawi LLC
// Test-only driver. Link against the pinned external OpenDroneID C library.
// No upstream implementation is included in the shipped engine.
#include <assert.h>
#include <stdio.h>
#include <string.h>
#include "opendroneid.h"

static void populate(ODID_UAS_Data *data, unsigned count) {
    odid_initUasData(data);
    data->BasicIDValid[0] = 1;
    data->BasicID[0].UAType = ODID_UATYPE_HELICOPTER_OR_MULTIROTOR;
    data->BasicID[0].IDType = ODID_IDTYPE_SERIAL_NUMBER;
    strcpy(data->BasicID[0].UASID, "NAN-ORACLE-01");
    if (count >= 2) {
        data->LocationValid = 1;
        data->Location.Status = ODID_STATUS_AIRBORNE;
        data->Location.Latitude = 40.7608;
        data->Location.Longitude = -95.3702;
        data->Location.AltitudeGeo = 321.5f;
        data->Location.AltitudeBaro = 320.0f;
    }
    if (count >= 3) {
        data->SystemValid = 1;
        data->System.OperatorLatitude = 40.7612;
        data->System.OperatorLongitude = -95.3706;
        data->System.OperatorAltitudeGeo = 280.5f;
    }
    if (count >= 4) {
        data->OperatorIDValid = 1;
        strcpy(data->OperatorID.OperatorId, "OPERATOR-TEST-01");
    }
    if (count >= 5) {
        data->SelfIDValid = 1;
        strcpy(data->SelfID.Desc, "NAN bench fixture");
    }
    for (unsigned page = 0; page + 5 < count; page++) {
        data->AuthValid[page] = 1;
        data->Auth[page].DataPage = page;
        data->Auth[page].AuthType = 1;
        data->Auth[page].LastPageIndex = count - 6;
        data->Auth[page].Length = 17 + 23 * (count - 6);
        data->Auth[page].Timestamp = 123456;
    }
}

int main(void) {
    const char mac[6] = {2, 1, 2, 3, 4, 5};
    const unsigned counters[] = {0, 239, 240, 255};
    unsigned emitted = 0;
    puts("{\"upstream_commit\":\"6484f26545d4f012682524e2d843fab0fbdc0b34\",\"upstream_license\":\"Apache-2.0\",\"cases\":[");
    for (unsigned count = 1; count <= 9; count++) {
        for (unsigned index = 0; index < sizeof(counters) / sizeof(counters[0]); index++) {
            unsigned counter = counters[index];
            ODID_UAS_Data input, decoded;
            populate(&input, count);
            uint8_t frame[512] = {0};
            int length = odid_wifi_build_message_pack_nan_action_frame(
                &input, mac, counter, frame, sizeof(frame));
            assert(length > 0);
            assert(frame[46] == count);
            odid_initUasData(&decoded);
            char received_mac[6];
            assert(odid_wifi_receive_message_pack_nan_action_frame(
                &decoded, received_mac, frame, length) == 0);
            assert(memcmp(mac, received_mac, sizeof(mac)) == 0);
            assert(strcmp(decoded.BasicID[0].UASID, input.BasicID[0].UASID) == 0);
            printf("%s{\"count\":%u,\"counter\":%u,\"frame_hex\":\"", emitted++ ? ",\n" : "", count, counter);
            for (int i = 0; i < length; i++) printf("%02x", frame[i]);
            printf("\",\"decoded\":{\"hardware_serial\":\"%s\"", decoded.BasicID[0].UASID);
            if (decoded.LocationValid) {
                printf(",\"operational_status\":%d,\"pos\":{\"lat\":%.7f,\"lon\":%.7f,\"alt_m\":%.1f}",
                    decoded.Location.Status, decoded.Location.Latitude, decoded.Location.Longitude,
                    decoded.Location.AltitudeGeo);
            }
            if (decoded.SystemValid) {
                printf(",\"operator_pos\":{\"lat\":%.7f,\"lon\":%.7f,\"alt_m\":%.1f}",
                    decoded.System.OperatorLatitude, decoded.System.OperatorLongitude,
                    decoded.System.OperatorAltitudeGeo);
            }
            if (decoded.OperatorIDValid) printf(",\"operator_id\":\"%s\"", decoded.OperatorID.OperatorId);
            if (decoded.SelfIDValid) printf(",\"self_id\":{\"desc_type\":%d,\"description\":\"%s\"}", decoded.SelfID.DescType, decoded.SelfID.Desc);
            if (decoded.AuthValid[0]) {
                printf(",\"auth\":{\"auth_type\":%d,\"timestamp_utc\":%lu,\"total_length\":%u,\"last_page_index\":%u}",
                    decoded.Auth[0].AuthType, 1546300800UL + decoded.Auth[0].Timestamp,
                    decoded.Auth[0].Length, decoded.Auth[0].LastPageIndex);
            }
            printf("}}");
        }
    }
    uint8_t sync[512] = {0};
    int sync_length = odid_wifi_build_nan_sync_beacon_frame(mac, sync, sizeof(sync));
    assert(sync_length > 36);
    memset(sync + 24, 0, 8); // Normalize the builder's clock for repeatable fixtures.
    printf("\n],\"sync_frame_hex\":\"");
    for (int i = 0; i < sync_length; i++) printf("%02x", sync[i]);
    puts("\"}");
    return 0;
}
