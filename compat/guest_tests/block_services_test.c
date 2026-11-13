/* SPDX-License-Identifier: GPL-2.0-only */
#if !KUTOX_GUEST_TESTS
#error "guest block fixture requires guest-tests"
#endif
#include "block/driver.h"

/* C request/PDU/scatterlist coverage; Rust owns submission and completion tests. */
bool kutox_c_block_request(struct request *req)
{
    if (blk_mq_rq_from_pdu(blk_mq_rq_to_pdu(req)) != req)
        return false;
    struct scatterlist sg;
    if (blk_rq_map_sg(req, &sg) != 1 || sg_next(&sg))
        return false;
    u8 *payload = (void *)(sg_page(&sg)->address + sg.offset);
    for (unsigned int i = 0; i < sg.length; i++)
        payload[i] = (u8)(i + blk_rq_pos(req));
    return sg.length == blk_rq_sectors(req) * 512;
}
