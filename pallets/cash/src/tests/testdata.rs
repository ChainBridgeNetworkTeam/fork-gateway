pub mod json_responses {
    pub const NO_RESULT: &[u8] = br#"{
        "jsonrpc":"2.0",
        "id":1,
        "result": null
    }"#;

    pub const GET_BLOCK_BY_NUMBER_2: &[u8] = br#"{
        "jsonrpc":"2.0",
        "id":1,
        "result":{
         "difficulty":"0xb9e274f7969f5",
         "extraData":"0x65746865726d696e652d657531",
         "gasLimit":"0x7a121d",
         "gasUsed":"0x781503",
         "hash":"0x50314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
         "logsBloom":"0x044410ea904e1020440110008000902200168801c81010301489212010002008080b0010004001b006040222c42004b001200408400500901889c908212040401020008d300010100198d10800100080027900254120000000530141030808140c299400162c0000d200204080008838240009002c020010400010101000481660200420a884b8020282204a00141ce10805004810800190180114180001b0001b1000020ac8040007000320b0480004018240891882a20080010281002c00000010102e0184210003010100438004202003080401000806204010000a42200104110100201200008081005001104002410140114a002010808c00200894c0c0",
         "miner":"0xea674fdde714fd979de3edf0f56aa9716b898ec8",
         "mixHash":"0xd733e12126a2155f0278c3987777eaca558a274b42d0396306dffb8fa6d21e76",
         "nonce":"0x56a66f3802150748",
         "number":"0x2",
         "parentHash":"0x61314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
         "receiptsRoot":"0x19ad317358916207491d4b64340153b924f4dda88fa8ef5dcb49090f234c00e7",
         "sha3Uncles":"0xd21bed33f01dac18a3ee5538d1607ff2709d742eb4e13877cf66dcbed6c980f2",
         "size":"0x5f50",
         "stateRoot":"0x40b48fa241b8f9749af10a5dd1dfb8db245ba94cbb4969ab5c5b905a6adfe5f6",
         "timestamp":"0x5aae89b9",
         "totalDifficulty":"0xa91291ae5c752d4885",
         "transactionsRoot":"0xa46bb7bc06d4ad700df4100095fecd5a5af2994b6d1d24162ded673b7d485610",
         "uncles":["0x5e7dde2e3811b5881a062c8b2ff7fd14687d79745e2384965d73a9df3fb0b4a8"]}
    }"#;

    pub const GET_BLOCK_BY_NUMBER_3: &[u8] = br#"{
        "jsonrpc":"2.0",
        "id":1,
        "result":{
         "difficulty":"0xb9e274f7969f5",
         "extraData":"0x65746865726d696e652d657531",
         "gasLimit":"0x7a121d",
         "gasUsed":"0x781503",
         "hash":"0x72314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
         "logsBloom":"0x044410ea904e1020440110008000902200168801c81010301489212010002008080b0010004001b006040222c42004b001200408400500901889c908212040401020008d300010100198d10800100080027900254120000000530141030808140c299400162c0000d200204080008838240009002c020010400010101000481660200420a884b8020282204a00141ce10805004810800190180114180001b0001b1000020ac8040007000320b0480004018240891882a20080010281002c00000010102e0184210003010100438004202003080401000806204010000a42200104110100201200008081005001104002410140114a002010808c00200894c0c0",
         "miner":"0xea674fdde714fd979de3edf0f56aa9716b898ec8",
         "mixHash":"0xd733e12126a2155f0278c3987777eaca558a274b42d0396306dffb8fa6d21e76",
         "nonce":"0x56a66f3802150748",
         "number":"0x3",
         "parentHash":"0x61314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
         "receiptsRoot":"0x19ad317358916207491d4b64340153b924f4dda88fa8ef5dcb49090f234c00e7",
         "sha3Uncles":"0xd21bed33f01dac18a3ee5538d1607ff2709d742eb4e13877cf66dcbed6c980f2",
         "size":"0x5f50",
         "stateRoot":"0x40b48fa241b8f9749af10a5dd1dfb8db245ba94cbb4969ab5c5b905a6adfe5f6",
         "timestamp":"0x5aae89b9",
         "totalDifficulty":"0xa91291ae5c752d4885",
         "transactionsRoot":"0xa46bb7bc06d4ad700df4100095fecd5a5af2994b6d1d24162ded673b7d485610",
         "uncles":["0x5e7dde2e3811b5881a062c8b2ff7fd14687d79745e2384965d73a9df3fb0b4a8"]}
    }"#;

    pub const GET_LOGS_2: &[u8] = br#"{
        "jsonrpc":"2.0",
        "id":1,
        "result": []
    }"#;

    pub const GET_LOGS_3: &[u8] = br#"{
        "jsonrpc":"2.0",
        "id":1,
        "result": [
            {
                "address":"0xbbde1662bc3ed16aa8c618c9833c801f3543b587",
                "blockHash":"0x72314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
                "blockNumber":"0x3",
                "data":"0x00000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000de0b6b3a764000000000000000000000000000000000000000000000000000000000000000000034554480000000000000000000000000000000000000000000000000000000000",
                "logIndex":"0x0",
                "removed":false,
                "topics":[
                    "0xc459acef3ffe957663bb49d644b20d0c790bcb41573893752a72ba6f023b9386",
                    "0x000000000000000000000000eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
                    "0x000000000000000000000000feb1ea27f888c384f1b0dc14fd6b387d5ff47031",
                    "0x513c1ff435eccedd0fda5edd2ad5e5461f0e8726000000000000000000000000"
                ],
                "transactionHash":"0x680e1e81385151f5d791fab0a3c06b03d29b46df08a312d0304cd6a4fc5a7370",
                "transactionIndex":"0x0"
            },
            {
                "address":"0xbbde1662bc3ed16aa8c618c9833c801f3543b587",
                "blockHash":"0x72314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
                "blockNumber":"0x3",
                "data":"0x00000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000de0b6b3a764000000000000000000000000000000000000000000000000000000000000000000034554480000000000000000000000000000000000000000000000000000000000",
                "logIndex":"0x1",
                "removed":false,
                "topics":[
                    "0xc459acef3ffe957663bb49d644b20d0c790bcb41573893752a72ba6f023b9386",
                    "0x000000000000000000000000d87ba7a50b2e7e660f678a895e4b72e7cb4ccd9c",
                    "0x000000000000000000000000feb1ea27f888c384f1b0dc14fd6b387d5ff47031",
                    "0xfeb1ea27f888c384f1b0dc14fd6b387d5ff47031000000000000000000000000"
                ],
                "transactionHash":"0x7357859bd05b4429dac758df67f93adb54caad72dd992317811927232c592d4a",
                "transactionIndex":"0x0"
            },
            {
                "address":"0xbbde1662bc3ed16aa8c618c9833c801f3543b587",
                "blockHash":"0x72314c1c6837e15e60c5b6732f092118dd25e3ec681f5e089b3a9ad2374e5a8a",
                "blockNumber":"0x3",
                "data":"0x00000000000000000000000000000000000000000000000000000000000000400000000000000000000000000000000000000000000000000de0b6b3a764000000000000000000000000000000000000000000000000000000000000000000034554480000000000000000000000000000000000000000000000000000000000",
                "logIndex":"0xe",
                "removed":false,
                "topics":[
                    "0xc459acef3ffe957663bb49d644b20d0c790bcb41573893752a72ba6f023b9386",
                    "0x000000000000000000000000e4e81fa6b16327d4b78cfeb83aade04ba7075165",
                    "0x000000000000000000000000feb1ea27f888c384f1b0dc14fd6b387d5ff47031",
                    "0xfeb1ea27f888c384f1b0dc14fd6b387d5ff47031000000000000000000000000"
                ],
                "transactionHash":"0xad28d82aa1f55e5f965c1da2d84cce29bdb75a134b8f7857c897736c4e562300",
                "transactionIndex":"0x4"
            }
        ]
    }"#;
}
